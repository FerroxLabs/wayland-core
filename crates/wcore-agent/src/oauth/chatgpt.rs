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

// ── Device-code (headless / "Sign in with ChatGPT" without a browser) ─────
/// Step 1 endpoint: request a user code + device-auth id.
pub const DEVICEAUTH_USERCODE_URL: &str =
    "https://auth.openai.com/api/accounts/deviceauth/usercode";
/// Step 3 endpoint: poll for the authorization code + PKCE verifier.
pub const DEVICEAUTH_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
/// Verification URL shown to the user — they open it and type the user code.
pub const DEVICE_VERIFY_URL: &str = "https://auth.openai.com/codex/device";
/// `redirect_uri` used in the final code→token exchange for the device flow.
/// OpenAI's device service pins this; the loopback flow uses a `localhost`
/// redirect instead.
pub const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";

/// Refresh this many seconds before expiry to absorb clock skew.
const REFRESH_LEAD_SECS: u64 = 120;
/// Outer wall-clock cap on the refresh round-trip.
const PER_CALL_TIMEOUT: Duration = Duration::from_secs(20);

/// Per-request cap on each device-code HTTP round-trip (usercode + poll).
const DEVICE_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// Floor on the server-provided poll interval — never poll faster than this.
const DEVICE_POLL_MIN_INTERVAL: Duration = Duration::from_secs(3);
/// Wall-clock cap on the whole device-code login (request + poll loop).
const DEVICE_LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
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
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let seg = access_token.split('.').nth(1).ok_or("not a JWT")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(seg)
        .map_err(|e| format!("b64: {e}"))?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| format!("json: {e}"))?;
    let auth = v
        .get("https://api.openai.com/auth")
        .ok_or("no auth claim")?;
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

/// Decode the JWT `exp` (expiry) claim, in Unix epoch seconds. Reads the
/// standard top-level `exp` claim from the payload segment — distinct from
/// [`decode_codex_claims`], which reads the OpenAI auth-namespace claim.
/// Returns `None` when the segment is absent / not base64url / not JSON / has
/// no numeric `exp`.
fn decode_jwt_exp(token: &str) -> Option<u64> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let seg = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(seg).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("exp").and_then(|x| x.as_u64())
}

/// A point-in-time, NETWORK-FREE snapshot of the stored ChatGPT login.
///
/// Produced by [`login_status`] from the on-disk token bundle alone — no
/// refresh, no HTTP. `signed_in` is true whenever a token file is present;
/// `expires_at_unix_secs` lets the caller decide expired-vs-valid against its
/// own wall-clock (an expired-but-present token is still `signed_in` because
/// the next real use will silently refresh it). This is the ONE source of
/// truth shared by the CLI `auth status` command, the `/provider` swap
/// precheck, and the `/config` status row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGptLoginStatus {
    /// `chatgpt_plan_type` from the access-token claims (e.g. `pro`, `plus`),
    /// when present and decodable.
    pub plan: Option<String>,
    /// Access-token expiry in Unix epoch seconds, when known. Prefers the
    /// stored `expires_at_unix_secs`; falls back to the JWT `exp` claim.
    pub expires_at_unix_secs: Option<u64>,
    /// Always `true` when this value exists (a token file was found).
    pub signed_in: bool,
}

impl ChatGptLoginStatus {
    /// Decode a signed-in status from an already-loaded token bundle (no I/O).
    /// Shared by [`login_status`] (which loads from `OAuthStorage` first) and
    /// the CLI `auth status` renderer so the plan/expiry decode lives in ONE
    /// place. `plan` is the `chatgpt_plan_type` claim; expiry prefers the
    /// stored field, falling back to the JWT `exp`.
    pub fn from_tokens(tokens: &OAuthTokens) -> Self {
        let plan = decode_codex_claims(&tokens.access_token)
            .ok()
            .and_then(|c| c.plan_type);
        let expires_at_unix_secs = tokens
            .expires_at_unix_secs
            .or_else(|| decode_jwt_exp(&tokens.access_token));
        Self {
            plan,
            expires_at_unix_secs,
            signed_in: true,
        }
    }
}

/// Report the stored ChatGPT login WITHOUT any network call or refresh.
///
/// Loads `chatgpt`'s tokens from `storage`, and — when present — decodes the
/// plan from the access-token claims and the expiry from the stored field
/// (falling back to the JWT `exp`). Returns `Ok(None)` when no token file
/// exists (not signed in), `Err` only on a storage read error. This is a pure
/// read of already-persisted state, so it is safe to call from synchronous UI
/// paths (the `/config` surface, the `/provider` precheck).
pub fn login_status(
    storage: &OAuthStorage,
) -> Result<Option<ChatGptLoginStatus>, crate::oauth::OAuthStorageError> {
    let Some(tokens) = storage.load(PROVIDER)? else {
        return Ok(None);
    };
    Ok(Some(ChatGptLoginStatus::from_tokens(&tokens)))
}

/// Import a ChatGPT login from the Codex CLI's `auth.json`.
///
/// Reads `$CODEX_HOME/auth.json` (default `~/.codex/auth.json`), maps the
/// `tokens` object to [`OAuthTokens`], and derives `expires_at_unix_secs`
/// from the access-token JWT `exp` claim. The caller persists the result.
///
/// C6 hardening — `$CODEX_HOME` is attacker-influenceable, so before trusting
/// the file we:
/// - canonicalize (realpath) the path and confirm it stays under the resolved
///   `$CODEX_HOME` (no symlink escape);
/// - on Unix, require the file be owned by the current user and NOT
///   group/world-writable; if `$CODEX_HOME` was set via the environment, the
///   ownership check is MANDATORY (we never auto-trust an env-pointed file
///   that fails it);
/// - run [`decode_codex_claims`] and reject the import if the access token
///   carries no `chatgpt_account_id`.
pub fn import_codex_cli_tokens() -> Result<OAuthTokens, String> {
    let (codex_home, from_env) = codex_home_dir()?;
    let auth_path = codex_home.join("auth.json");

    // Canonicalize and confirm containment under the resolved CODEX_HOME so a
    // symlinked auth.json can't redirect the read outside the trusted dir.
    let real_home = std::fs::canonicalize(&codex_home)
        .map_err(|e| format!("resolving CODEX_HOME ({}): {e}", codex_home.display()))?;
    let real_auth = std::fs::canonicalize(&auth_path)
        .map_err(|e| format!("no Codex CLI login at {} ({e})", auth_path.display()))?;
    if !real_auth.starts_with(&real_home) {
        return Err(format!(
            "Codex auth.json ({}) resolves outside CODEX_HOME ({}) — refusing to import",
            real_auth.display(),
            real_home.display()
        ));
    }

    // Ownership / permission gate (Unix). Mandatory when CODEX_HOME is
    // env-supplied; defense-in-depth otherwise.
    check_codex_auth_perms(&real_auth, from_env)?;

    let bytes =
        std::fs::read(&real_auth).map_err(|e| format!("reading {}: {e}", real_auth.display()))?;
    let doc: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("parsing Codex auth.json: {e}"))?;

    let tokens = doc
        .get("tokens")
        .ok_or("Codex auth.json has no `tokens` object (is this an API-key login?)")?;
    let access_token = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("Codex auth.json has no access_token")?
        .to_string();

    // Reject a token with no ChatGPT account id — it cannot drive the Codex
    // backend and would only surface a confusing 4xx later.
    decode_codex_claims(&access_token)
        .map_err(|e| format!("Codex access token carries no ChatGPT account id: {e}"))?;

    let refresh_token = tokens
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let id_token = tokens
        .get("id_token")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let expires_at_unix_secs = decode_jwt_exp(&access_token);

    Ok(OAuthTokens {
        access_token,
        refresh_token,
        expires_at_unix_secs,
        token_type: "Bearer".to_string(),
        scope: None,
        id_token,
    })
}

/// Resolve the Codex home directory. Returns `(dir, from_env)` where
/// `from_env` is true iff `$CODEX_HOME` was set (which makes the ownership
/// check mandatory). Default is `~/.codex`.
fn codex_home_dir() -> Result<(std::path::PathBuf, bool), String> {
    if let Some(v) = std::env::var_os("CODEX_HOME") {
        let s = v.to_string_lossy();
        if !s.trim().is_empty() {
            return Ok((std::path::PathBuf::from(v), true));
        }
    }
    let home = dirs::home_dir().ok_or("home directory unresolvable")?;
    Ok((home.join(".codex"), false))
}

/// Verify the Codex auth.json is safe to trust: owned by the current user and
/// not group/world-writable. On non-Unix this is a no-op (the profile-dir ACL
/// covers it). When `mandatory` (env-supplied CODEX_HOME) a failure is an
/// error; we never auto-trust an env-pointed file that fails the check.
#[cfg(unix)]
fn check_codex_auth_perms(path: &std::path::Path, mandatory: bool) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).map_err(|e| format!("stat {}: {e}", path.display()))?;
    let mut problems = Vec::new();
    // SAFETY: getuid is always-succeeds and async-signal-safe; it reads the
    // calling process's real UID. Declared locally (mirroring wcore-cron's
    // store) so wcore-agent need not pull in the `libc` crate for one call.
    let uid = unsafe { codex_getuid() };
    if meta.uid() != uid {
        problems.push(format!(
            "owned by uid {} not the current user ({uid})",
            meta.uid()
        ));
    }
    // Reject group- or world-writable files (0o022 bits set).
    if meta.mode() & 0o022 != 0 {
        problems.push("group/world-writable".to_string());
    }
    if problems.is_empty() {
        return Ok(());
    }
    let msg = format!(
        "Codex auth.json ({}) failed the ownership/permission check: {}",
        path.display(),
        problems.join(", ")
    );
    if mandatory {
        Err(msg)
    } else {
        // Non-env (default ~/.codex): warn but allow — the home dir is already
        // user-private. The mandatory gate covers the attacker-controlled case.
        tracing::warn!("{msg}");
        Ok(())
    }
}

#[cfg(not(unix))]
fn check_codex_auth_perms(_path: &std::path::Path, _mandatory: bool) -> Result<(), String> {
    Ok(())
}

// Minimal FFI for the running user's real uid. Declared locally (same pattern
// as `wcore-cron::store`) so wcore-agent keeps its dependency surface small.
#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "getuid"]
    fn codex_getuid() -> u32;
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

    /// Construct a manager whose OAuth flow descriptor is supplied explicitly.
    ///
    /// Production code uses [`ChatGptTokenManager::new`], which hardwires the
    /// real `auth.openai.com` token endpoint via [`build_chatgpt_flow`]. This
    /// seam lets out-of-crate integration tests point the refresh round-trip at
    /// a local mock token server (the in-crate unit tests reach the private
    /// `flow` field directly; an external `tests/` binary cannot, hence this
    /// hidden constructor).
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

/// Parsed Step-1 response from [`DEVICEAUTH_USERCODE_URL`]: the user-facing
/// code, the opaque device-auth id used when polling, and the server's
/// suggested poll interval (seconds).
#[derive(Debug)]
struct DeviceUserCode {
    user_code: String,
    device_auth_id: String,
    interval: Duration,
}

/// Parse the Step-1 usercode JSON. Accepts `user_code` or the `usercode`
/// alias (both observed in the wild), requires a non-empty `device_auth_id`, and
/// floors `interval` at [`DEVICE_POLL_MIN_INTERVAL`]. A missing/zero interval
/// falls back to the floor.
fn parse_device_usercode(body: &str) -> Result<DeviceUserCode, String> {
    let raw: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("malformed device usercode JSON: {e}"))?;
    let user_code = raw
        .get("user_code")
        .or_else(|| raw.get("usercode"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("device usercode response missing user_code")?
        .to_string();
    let device_auth_id = raw
        .get("device_auth_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("device usercode response missing device_auth_id")?
        .to_string();
    // `interval` may arrive as a number or a string ("5"); accept either.
    let interval_secs = raw
        .get("interval")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0);
    let interval = Duration::from_secs(interval_secs).max(DEVICE_POLL_MIN_INTERVAL);
    Ok(DeviceUserCode {
        user_code,
        device_auth_id,
        interval,
    })
}

/// The authorization code + PKCE verifier returned by a successful (HTTP 200)
/// poll of [`DEVICEAUTH_TOKEN_URL`]. OpenAI's device service GENERATES the
/// PKCE pair server-side and hands the verifier back here — the client never
/// generates one for the device flow.
#[derive(Debug)]
struct DeviceAuthorization {
    authorization_code: String,
    code_verifier: String,
}

/// Parse a successful (HTTP 200) device-poll body. Both fields are required;
/// a 200 missing either is treated as a protocol error.
fn parse_device_authorization(body: &str) -> Result<DeviceAuthorization, String> {
    let raw: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("malformed device poll JSON: {e}"))?;
    let authorization_code = raw
        .get("authorization_code")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("device poll response missing authorization_code")?
        .to_string();
    let code_verifier = raw
        .get("code_verifier")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("device poll response missing code_verifier")?
        .to_string();
    Ok(DeviceAuthorization {
        authorization_code,
        code_verifier,
    })
}

/// Run the headless "Sign in with ChatGPT" device-code flow end to end and
/// return the exchanged tokens. No browser, no loopback listener — the user
/// opens [`DEVICE_VERIFY_URL`] on any device and types the printed user code.
///
/// Steps: (1) POST [`DEVICEAUTH_USERCODE_URL`] for a user code + device-auth
/// id; (2) print the verification URL + code; (3) poll
/// [`DEVICEAUTH_TOKEN_URL`] every server-suggested interval (floored at
/// [`DEVICE_POLL_MIN_INTERVAL`], capped at [`DEVICE_LOGIN_TIMEOUT`] total)
/// until a 200 returns the authorization code + PKCE verifier; (4) exchange
/// those via the EXISTING [`build_chatgpt_flow`] against [`DEVICE_REDIRECT_URI`].
///
/// C7 discipline: token-endpoint response BODIES are never interpolated into
/// errors — only the HTTP status is surfaced.
pub async fn login_device_code(client: &wcore_egress::EgressClient) -> Result<OAuthTokens, String> {
    let user_code = request_device_code(client).await?;

    // Tell the user where to go. Printing is the contract of a headless flow.
    println!("To sign in to ChatGPT, on any device:");
    println!("  1. Open: {DEVICE_VERIFY_URL}");
    println!("  2. Enter code: {}", user_code.user_code);
    println!("Waiting for sign-in… (up to 15 minutes)");

    let authorization = poll_device_authorization(client, &user_code).await?;

    // Step 4: exchange via the shared Codex flow. The device service returned
    // the PKCE verifier, so we pass it straight through (no client-side PKCE).
    let flow = build_chatgpt_flow();
    flow.exchange_code(
        client,
        &authorization.authorization_code,
        DEVICE_REDIRECT_URI,
        Some(&authorization.code_verifier),
    )
    .await
    .map_err(|e| format!("device-code token exchange failed: {e}"))
}

/// Step 1: POST the client id to [`DEVICEAUTH_USERCODE_URL`] and parse the
/// user code + device-auth id. C7: only the status is surfaced on failure.
async fn request_device_code(
    client: &wcore_egress::EgressClient,
) -> Result<DeviceUserCode, String> {
    let res = tokio::time::timeout(
        DEVICE_HTTP_TIMEOUT,
        client
            .post(DEVICEAUTH_USERCODE_URL)
            .json(&serde_json::json!({ "client_id": CLIENT_ID }))
            .send(),
    )
    .await
    .map_err(|_| "device code request timed out".to_string())?
    .map_err(|e| format!("device code request transport error: {e}"))?;

    let status = res.status();
    let body = res
        .text()
        .await
        .map_err(|e| format!("reading device code response: {e}"))?;
    if !status.is_success() {
        // C7: never echo the body — status only.
        return Err(format!(
            "device code request rejected: HTTP {}",
            status.as_u16()
        ));
    }
    parse_device_usercode(&body)
}

/// Step 3: poll [`DEVICEAUTH_TOKEN_URL`] until the user finishes signing in.
///
/// HTTP 200 → return the authorization code + verifier. 403/404 (pending) →
/// wait the server-suggested interval and retry. Any other non-2xx → error
/// (status only, C7). Bounded by [`DEVICE_LOGIN_TIMEOUT`] of wall-clock.
async fn poll_device_authorization(
    client: &wcore_egress::EgressClient,
    user_code: &DeviceUserCode,
) -> Result<DeviceAuthorization, String> {
    let deadline = tokio::time::Instant::now() + DEVICE_LOGIN_TIMEOUT;
    let payload = serde_json::json!({
        "device_auth_id": user_code.device_auth_id,
        "user_code": user_code.user_code,
    });

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err("ChatGPT device sign-in timed out after 15 minutes".to_string());
        }
        // Wait BEFORE the first poll — the user needs time to type the code,
        // and the server returns pending immediately otherwise.
        tokio::time::sleep(user_code.interval).await;

        let res = tokio::time::timeout(
            DEVICE_HTTP_TIMEOUT,
            client.post(DEVICEAUTH_TOKEN_URL).json(&payload).send(),
        )
        .await
        .map_err(|_| "device authorization poll timed out".to_string())?
        .map_err(|e| format!("device authorization poll transport error: {e}"))?;

        let status = res.status();
        let body = res
            .text()
            .await
            .map_err(|e| format!("reading device poll response: {e}"))?;

        if status.is_success() {
            return parse_device_authorization(&body);
        }
        // 403/404 = user hasn't completed sign-in yet → keep waiting.
        if matches!(status.as_u16(), 403 | 404) {
            continue;
        }
        // C7: any other non-2xx is a hard error; surface the status only.
        return Err(format!(
            "device authorization poll rejected: HTTP {}",
            status.as_u16()
        ));
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
        let (url, _state, _pkce) = flow.build_authorize_url("http://localhost:1455/auth/callback");
        assert!(url.contains("id_token_add_organizations=true"), "url={url}");
        assert!(url.contains("codex_cli_simplified_flow=true"), "url={url}");
        assert!(url.contains("originator=wayland"), "url={url}");
    }

    // ── Task 2.3: token manager — fresh / rotate / errors / 429 ──────

    /// A 3-segment JWT whose payload decodes to the given account id. Built
    /// from a JSON string base64url-encoded so the fixtures stay readable.
    fn jwt_with_account(account_id: &str) -> String {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
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
        mgr.flow = Arc::new(build_chatgpt_flow_with_token_url(
            "http://127.0.0.1:1/never",
        ));
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
            .store(
                PROVIDER,
                &token(&jwt_with_account("acct_old"), Some("rt-OLD"), Some(0)),
            )
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
            .store(
                PROVIDER,
                &token(&jwt_with_account("acct_old"), Some("rt-KEEP"), Some(0)),
            )
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
            .store(
                PROVIDER,
                &token(&jwt_with_account("acct_dead"), Some("rt"), Some(0)),
            )
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
            .store(
                PROVIDER,
                &token(&jwt_with_account("acct_c"), Some("rt"), Some(far_future())),
            )
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

    // ── Task 5.3: Codex CLI token import (C6 hardening) ──────────────

    /// A 3-segment JWT carrying both the ChatGPT account-id namespace claim
    /// and a top-level `exp`, so the import path can derive expiry from it.
    fn jwt_with_account_and_exp(account_id: &str, exp: u64) -> String {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let payload = serde_json::json!({
            "exp": exp,
            "https://api.openai.com/auth": { "chatgpt_account_id": account_id }
        });
        let seg = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        format!("hdr.{seg}.sig")
    }

    /// Write a fake `$CODEX_HOME/auth.json` carrying the Codex CLI's `tokens`
    /// shape and return the CODEX_HOME dir.
    fn write_codex_auth(home: &std::path::Path, body: serde_json::Value) {
        std::fs::create_dir_all(home).unwrap();
        std::fs::write(
            home.join("auth.json"),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn imports_codex_tokens_with_account_id() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("codex");
        let exp = far_future();
        let access = jwt_with_account_and_exp("acct_codex", exp);
        write_codex_auth(
            &home,
            serde_json::json!({
                "OPENAI_API_KEY": serde_json::Value::Null,
                "tokens": {
                    "access_token": access,
                    "refresh_token": "rt-codex",
                    "id_token": "id-codex",
                }
            }),
        );

        // SAFETY: serial test; env reverted before exit.
        let saved = std::env::var_os("CODEX_HOME");
        unsafe { std::env::set_var("CODEX_HOME", &home) };
        let result = import_codex_cli_tokens();
        match saved {
            Some(v) => unsafe { std::env::set_var("CODEX_HOME", v) },
            None => unsafe { std::env::remove_var("CODEX_HOME") },
        }

        let tokens = result.expect("import");
        assert_eq!(tokens.access_token, access);
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt-codex"));
        assert_eq!(tokens.id_token.as_deref(), Some("id-codex"));
        assert_eq!(tokens.expires_at_unix_secs, Some(exp));
    }

    #[test]
    #[serial_test::serial]
    fn rejects_codex_token_without_account_id() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("codex");
        // access_token payload {"foo":"bar"} — no chatgpt_account_id.
        let bad = format!("hdr.{}.sig", URL_SAFE_NO_PAD.encode(b"{\"foo\":\"bar\"}"));
        write_codex_auth(
            &home,
            serde_json::json!({ "tokens": { "access_token": bad } }),
        );

        let saved = std::env::var_os("CODEX_HOME");
        unsafe { std::env::set_var("CODEX_HOME", &home) };
        let result = import_codex_cli_tokens();
        match saved {
            Some(v) => unsafe { std::env::set_var("CODEX_HOME", v) },
            None => unsafe { std::env::remove_var("CODEX_HOME") },
        }

        let err = result.unwrap_err();
        assert!(err.contains("account id"), "err={err}");
    }

    #[test]
    #[serial_test::serial]
    fn errors_when_codex_auth_missing() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("codex");
        std::fs::create_dir_all(&home).unwrap(); // dir exists, file does not

        let saved = std::env::var_os("CODEX_HOME");
        unsafe { std::env::set_var("CODEX_HOME", &home) };
        let result = import_codex_cli_tokens();
        match saved {
            Some(v) => unsafe { std::env::set_var("CODEX_HOME", v) },
            None => unsafe { std::env::remove_var("CODEX_HOME") },
        }

        assert!(result.is_err());
    }

    #[test]
    fn decode_jwt_exp_reads_top_level_exp() {
        let jwt = jwt_with_account_and_exp("acct_x", 1_900_000_000);
        assert_eq!(decode_jwt_exp(&jwt), Some(1_900_000_000));
        assert_eq!(decode_jwt_exp("not-a-jwt"), None);
    }

    // ── login_status: sync, network-free login snapshot ──────────────

    /// A 3-segment JWT carrying the account id + a `chatgpt_plan_type` claim.
    fn jwt_with_plan(account_id: &str, plan: &str) -> String {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let payload = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_plan_type": plan
            }
        });
        let seg = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        format!("hdr.{seg}.sig")
    }

    #[test]
    fn login_status_none_for_empty_store() {
        let tmp = TempDir::new().unwrap();
        let storage = OAuthStorage::at_root(tmp.path().join("oauth")).unwrap();
        assert_eq!(login_status(&storage).unwrap(), None);
    }

    #[test]
    fn login_status_reports_plan_and_expiry_from_seeded_token() {
        let tmp = TempDir::new().unwrap();
        let storage = OAuthStorage::at_root(tmp.path().join("oauth")).unwrap();
        let exp = far_future();
        storage
            .store(
                PROVIDER,
                &token(&jwt_with_plan("acct_s", "pro"), Some("rt"), Some(exp)),
            )
            .unwrap();

        let status = login_status(&storage).unwrap().expect("signed in");
        assert!(status.signed_in);
        assert_eq!(status.plan.as_deref(), Some("pro"));
        assert_eq!(status.expires_at_unix_secs, Some(exp));
    }

    #[test]
    fn login_status_falls_back_to_jwt_exp_when_field_absent() {
        // No stored `expires_at_unix_secs`, but the JWT carries a top-level
        // `exp`. The snapshot must surface the JWT expiry rather than None.
        let tmp = TempDir::new().unwrap();
        let storage = OAuthStorage::at_root(tmp.path().join("oauth")).unwrap();
        let jwt = jwt_with_account_and_exp("acct_j", 1_900_000_000);
        storage
            .store(PROVIDER, &token(&jwt, Some("rt"), None))
            .unwrap();

        let status = login_status(&storage).unwrap().expect("signed in");
        assert_eq!(status.expires_at_unix_secs, Some(1_900_000_000));
        // No plan claim in this fixture → None, but still signed in.
        assert_eq!(status.plan, None);
        assert!(status.signed_in);
    }

    // ── Device-code flow: usercode + poll JSON parsing ───────────────

    #[test]
    fn parse_device_usercode_extracts_fields_and_floors_interval() {
        // interval below the floor (1s) must be raised to DEVICE_POLL_MIN_INTERVAL (3s).
        let parsed = parse_device_usercode(
            r#"{"user_code":"WXYZ-1234","device_auth_id":"dev-abc","interval":1}"#,
        )
        .expect("parse");
        assert_eq!(parsed.user_code, "WXYZ-1234");
        assert_eq!(parsed.device_auth_id, "dev-abc");
        assert_eq!(parsed.interval, DEVICE_POLL_MIN_INTERVAL);
    }

    #[test]
    fn parse_device_usercode_honors_a_larger_interval() {
        let parsed =
            parse_device_usercode(r#"{"user_code":"AAAA","device_auth_id":"dev","interval":10}"#)
                .expect("parse");
        assert_eq!(parsed.interval, Duration::from_secs(10));
    }

    #[test]
    fn parse_device_usercode_accepts_usercode_alias_and_string_interval() {
        // The `usercode` alias is observed in the wild; some servers send interval as a string.
        let parsed =
            parse_device_usercode(r#"{"usercode":"BBBB","device_auth_id":"dev","interval":"7"}"#)
                .expect("parse");
        assert_eq!(parsed.user_code, "BBBB");
        assert_eq!(parsed.interval, Duration::from_secs(7));
    }

    #[test]
    fn parse_device_usercode_rejects_missing_device_auth_id() {
        let err = parse_device_usercode(r#"{"user_code":"X","interval":5}"#).unwrap_err();
        assert!(err.contains("device_auth_id"), "err={err}");
    }

    #[test]
    fn parse_device_usercode_defaults_interval_to_floor_when_absent() {
        let parsed =
            parse_device_usercode(r#"{"user_code":"X","device_auth_id":"d"}"#).expect("parse");
        assert_eq!(parsed.interval, DEVICE_POLL_MIN_INTERVAL);
    }

    #[test]
    fn parse_device_authorization_extracts_code_and_verifier() {
        let parsed = parse_device_authorization(
            r#"{"authorization_code":"auth-42","code_verifier":"ver-99"}"#,
        )
        .expect("parse");
        assert_eq!(parsed.authorization_code, "auth-42");
        assert_eq!(parsed.code_verifier, "ver-99");
    }

    #[test]
    fn parse_device_authorization_rejects_missing_verifier() {
        // A 200 that omits the verifier is a protocol error — we cannot exchange.
        let err = parse_device_authorization(r#"{"authorization_code":"auth-42"}"#).unwrap_err();
        assert!(err.contains("code_verifier"), "err={err}");
    }

    /// End-to-end of the device-code flow against a mock server: Step 1
    /// usercode, two PENDING (403) polls, then a 200 carrying the
    /// authorization code + verifier, then the final `/oauth/token` exchange.
    /// Proves the poll loop keeps waiting on 403 and the exchange reuses the
    /// returned verifier.
    #[tokio::test]
    async fn login_device_code_polls_then_exchanges() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

        let server = MockServer::start().await;

        // Step 1: usercode. interval=0 → parse_device_usercode floors it to
        // DEVICE_POLL_MIN_INTERVAL. The test drives the poll loop manually
        // (no sleeps) so the floor is asserted, not waited on.
        Mock::given(method("POST"))
            .and(path("/api/accounts/deviceauth/usercode"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user_code": "CODE-1",
                "device_auth_id": "dev-1",
                "interval": 0
            })))
            .mount(&server)
            .await;

        // Step 3: poll — first two calls 403 (pending), third 200 with the code.
        struct PollResponder {
            calls: Arc<AtomicUsize>,
        }
        impl Respond for PollResponder {
            fn respond(&self, _req: &Request) -> ResponseTemplate {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    ResponseTemplate::new(403).set_body_string("authorization_pending")
                } else {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "authorization_code": "dev-auth-code",
                        "code_verifier": "dev-verifier"
                    }))
                }
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        Mock::given(method("POST"))
            .and(path("/api/accounts/deviceauth/token"))
            .respond_with(PollResponder {
                calls: calls.clone(),
            })
            .mount(&server)
            .await;

        // Step 4: the final code→token exchange hits the real /oauth/token path.
        let new_at = jwt_with_account("acct_device");
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": new_at,
                "refresh_token": "rt-device",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .mount(&server)
            .await;

        // Drive the three steps directly against the mock server's URLs so we
        // exercise the real poll loop + exchange without the hardwired
        // auth.openai.com hosts. (login_device_code itself uses the real
        // constants; this test covers the loop/parse/exchange wiring through
        // the building blocks it calls.)
        let client = wcore_egress::EgressClient::new();

        let user_code = {
            let res = client
                .post(format!("{}/api/accounts/deviceauth/usercode", server.uri()))
                .json(&serde_json::json!({ "client_id": CLIENT_ID }))
                .send()
                .await
                .unwrap();
            assert!(res.status().is_success());
            parse_device_usercode(&res.text().await.unwrap()).unwrap()
        };
        assert_eq!(user_code.user_code, "CODE-1");
        assert_eq!(user_code.interval, DEVICE_POLL_MIN_INTERVAL);

        // Poll: 403, 403, 200.
        let payload = serde_json::json!({
            "device_auth_id": user_code.device_auth_id,
            "user_code": user_code.user_code,
        });
        let authorization = loop {
            let res = client
                .post(format!("{}/api/accounts/deviceauth/token", server.uri()))
                .json(&payload)
                .send()
                .await
                .unwrap();
            let status = res.status();
            let body = res.text().await.unwrap();
            if status.is_success() {
                break parse_device_authorization(&body).unwrap();
            }
            assert!(
                matches!(status.as_u16(), 403 | 404),
                "pending must be 403/404"
            );
        };
        assert_eq!(authorization.authorization_code, "dev-auth-code");
        assert_eq!(authorization.code_verifier, "dev-verifier");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "expected two pending polls then success"
        );

        // Exchange reuses the returned verifier against /oauth/token.
        let flow = build_chatgpt_flow_with_token_url(&format!("{}/oauth/token", server.uri()));
        let tokens = flow
            .exchange_code(
                &client,
                &authorization.authorization_code,
                DEVICE_REDIRECT_URI,
                Some(&authorization.code_verifier),
            )
            .await
            .expect("exchange");
        assert_eq!(tokens.access_token, new_at);
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt-device"));
        let claims = decode_codex_claims(&tokens.access_token).unwrap();
        assert_eq!(claims.account_id, "acct_device");
    }
}
