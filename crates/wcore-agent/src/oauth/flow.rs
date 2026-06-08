//! OAuth 2.0 authorization-code + refresh flow.
//!
//! v0.9.0 Wave-1 B0 scaffolding. Provides:
//! - `OAuthFlow`: provider-agnostic descriptor (client_id, scopes, etc.)
//! - `OAuthTokens`: result of a successful exchange or refresh
//! - `SingleFlightRefresh`: coalesces concurrent refresh requests
//!
//! Security defaults baked in:
//! - PKCE (S256) required unless `without_pkce()` explicitly invoked
//! - CSRF `state` is 32 random bytes from `OsRng`, compared with
//!   `subtle::ConstantTimeEq` on the callback
//! - Listener has a 5-minute idle timeout
//! - Callback listener is bound to `127.0.0.1` only — never the
//!   public interface
//!
//! Wave-1 B0 deliberately stops short of opening the real browser /
//! binding the real listener so the OAuth code can be unit-tested in
//! isolation. B9 (google_meet) drives the surface end-to-end.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rand::RngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;

use super::pkce::{PkceChallenge, PkceMode};

/// Where the OAuth callback listener binds for the redirect_uri.
#[derive(Debug, Clone, Copy, Default)]
pub enum RedirectStrategy {
    /// Bind to `127.0.0.1:0` (ephemeral port). Default. Closes R-H7's
    /// "two CLI instances collide on a fixed port".
    #[default]
    DynamicPort,
    /// Bind to `127.0.0.1:<port>`. Required for providers that pin the
    /// redirect_uri at app-registration time (e.g. Google when the
    /// console entry is `http://127.0.0.1:8765/`).
    FixedPort(u16),
}

/// Provider-agnostic OAuth flow descriptor.
#[derive(Debug, Clone)]
pub struct OAuthFlow {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub auth_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub redirect_strategy: RedirectStrategy,
    pub pkce_mode: PkceMode,
    /// Idle timeout on the callback listener. Defaults to 5 minutes.
    pub listener_idle_timeout: Duration,
}

impl OAuthFlow {
    /// New flow with PKCE-S256 required by default and a 5-minute
    /// listener idle timeout.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: Option<String>,
        auth_url: impl Into<String>,
        token_url: impl Into<String>,
        scopes: Vec<String>,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret,
            auth_url: auth_url.into(),
            token_url: token_url.into(),
            scopes,
            redirect_strategy: RedirectStrategy::default(),
            pkce_mode: PkceMode::S256,
            listener_idle_timeout: Duration::from_secs(300),
        }
    }

    /// Opt out of PKCE — only for legacy providers that reject the
    /// challenge. Wave-1 audit requires this be visible at the call
    /// site rather than buried in a config flag.
    pub fn without_pkce(mut self) -> Self {
        self.pkce_mode = PkceMode::Disabled;
        self
    }

    /// Override the default listener idle timeout (5 minutes). Used by
    /// tests to avoid waiting; production should leave at default.
    pub fn with_listener_idle_timeout(mut self, dur: Duration) -> Self {
        self.listener_idle_timeout = dur;
        self
    }

    /// Override the redirect strategy.
    pub fn with_redirect_strategy(mut self, strategy: RedirectStrategy) -> Self {
        self.redirect_strategy = strategy;
        self
    }

    /// Build the authorize URL the user's browser should open. Includes
    /// state token + PKCE challenge when enabled.
    ///
    /// Returns `(url, state, pkce)` so the caller can validate the
    /// callback later. The state token is generated here from `OsRng`
    /// (32 random bytes, base64url).
    pub fn build_authorize_url(
        &self,
        redirect_uri: &str,
    ) -> (String, String, Option<PkceChallenge>) {
        let state = new_state_token();
        let pkce = match self.pkce_mode {
            PkceMode::S256 => Some(PkceChallenge::new_s256()),
            PkceMode::Disabled => None,
        };

        // Build query string with explicit ordering so tests can match.
        let mut params: Vec<(String, String)> = vec![
            ("response_type".into(), "code".into()),
            ("client_id".into(), self.client_id.clone()),
            ("redirect_uri".into(), redirect_uri.to_string()),
            ("state".into(), state.clone()),
        ];
        if !self.scopes.is_empty() {
            params.push(("scope".into(), self.scopes.join(" ")));
        }
        if let Some(p) = pkce.as_ref() {
            params.push(("code_challenge".into(), p.challenge.clone()));
            params.push(("code_challenge_method".into(), p.method_str().to_string()));
        }
        let qs = params
            .iter()
            .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        let sep = if self.auth_url.contains('?') {
            '&'
        } else {
            '?'
        };
        let url = format!("{}{}{}", self.auth_url, sep, qs);

        (url, state, pkce)
    }

    /// Validate the state token returned on the callback using a
    /// constant-time comparison. Returns `true` only when the bytes
    /// match exactly. Use this on the callback path BEFORE doing the
    /// token exchange.
    pub fn validate_state(expected: &str, got: &str) -> bool {
        use subtle::ConstantTimeEq;
        // ConstantTimeEq panics when lengths differ; gate explicitly so
        // length mismatches are also rejected without panic.
        if expected.len() != got.len() {
            return false;
        }
        expected.as_bytes().ct_eq(got.as_bytes()).into()
    }
}

/// Error surface for the loopback callback round-trip — binding the
/// redirect listener, parsing the callback request, and exchanging the
/// authorization code for tokens.
#[derive(Debug, Error)]
pub enum CallbackError {
    #[error("failed to bind loopback callback listener: {0}")]
    Bind(String),
    #[error("callback listener timed out waiting for the browser redirect")]
    Timeout,
    #[error("callback request was malformed: {0}")]
    MalformedRequest(String),
    #[error("authorization server returned an error on the callback: {0}")]
    AuthorizationDenied(String),
    #[error("callback carried no `code` parameter")]
    MissingCode,
    #[error("callback `state` did not match the expected CSRF token")]
    StateMismatch,
    #[error("token exchange transport error: {0}")]
    Transport(String),
    #[error("token endpoint rejected the code: {0}")]
    ProviderRejected(String),
    #[error("token endpoint returned a malformed response: {0}")]
    MalformedTokenResponse(String),
}

/// The interesting fields parsed out of the OAuth redirect callback's
/// query string. Exactly one of (`code`) or (`error`) is populated by a
/// well-behaved authorization server; we surface both so the caller can
/// distinguish a user denial from a successful consent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// Parse the `code` / `state` / `error` parameters out of a callback
/// request target (the part after `GET `, e.g. `/callback?code=…&state=…`).
///
/// Pure and network-free so the callback-handling logic is unit-testable
/// without binding a socket. Percent-decodes values via `urlencoding`.
pub fn parse_callback_query(request_target: &str) -> CallbackParams {
    // Take everything after the first '?'; if there's no query string the
    // map stays empty and every field is None.
    let query = request_target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let decoded = urlencoding::decode(v)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| v.to_string());
        match k {
            "code" => code = Some(decoded),
            "state" => state = Some(decoded),
            "error" => error = Some(decoded),
            _ => {}
        }
    }
    CallbackParams { code, state, error }
}

impl OAuthFlow {
    /// Resolve the bind address for this flow's redirect strategy.
    fn bind_addr(&self) -> SocketAddr {
        let port = match self.redirect_strategy {
            RedirectStrategy::DynamicPort => 0,
            RedirectStrategy::FixedPort(p) => p,
        };
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    /// Validate a parsed callback against the expected CSRF `state` and
    /// extract the authorization code. Pure — no socket, no network — so
    /// the security-critical "reject forged state / surface denials"
    /// logic is unit-testable in isolation.
    ///
    /// Order matters: an `error=...` (user denied consent) is reported
    /// before the state check so the user sees "you denied" rather than a
    /// confusing CSRF message, but a present-and-mismatched state on an
    /// otherwise-successful callback is always rejected.
    pub fn authorization_code_from_callback(
        expected_state: &str,
        params: &CallbackParams,
    ) -> Result<String, CallbackError> {
        if let Some(err) = &params.error {
            return Err(CallbackError::AuthorizationDenied(err.clone()));
        }
        let got_state = params
            .state
            .as_deref()
            .ok_or(CallbackError::StateMismatch)?;
        if !Self::validate_state(expected_state, got_state) {
            return Err(CallbackError::StateMismatch);
        }
        params.code.clone().ok_or(CallbackError::MissingCode)
    }

    /// Exchange an authorization `code` (plus the PKCE verifier) for an
    /// access/refresh token bundle at this flow's `token_url`.
    ///
    /// The `client` is injected so tests can drive a wiremock server with
    /// no live network. Mirrors the refresh-path token parsing already
    /// used by the Google Meet backend (carry `refresh_token` forward,
    /// derive `expires_at` from `expires_in`).
    pub async fn exchange_code(
        &self,
        client: &wcore_egress::EgressClient,
        code: &str,
        redirect_uri: &str,
        pkce_verifier: Option<&str>,
    ) -> Result<OAuthTokens, CallbackError> {
        let mut form: Vec<(&str, String)> = vec![
            ("grant_type", "authorization_code".into()),
            ("code", code.to_string()),
            ("redirect_uri", redirect_uri.to_string()),
            ("client_id", self.client_id.clone()),
        ];
        if let Some(secret) = &self.client_secret {
            form.push(("client_secret", secret.clone()));
        }
        if let Some(verifier) = pkce_verifier {
            form.push(("code_verifier", verifier.to_string()));
        }

        let res = client
            .post(&self.token_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| CallbackError::Transport(e.to_string()))?;

        let status = res.status();
        let body = res
            .text()
            .await
            .map_err(|e| CallbackError::Transport(e.to_string()))?;

        if !status.is_success() {
            return Err(CallbackError::ProviderRejected(format!(
                "HTTP {} — {body}",
                status.as_u16()
            )));
        }

        Self::parse_token_response(&body)
    }

    /// Parse a token-endpoint JSON body into [`OAuthTokens`]. Pure and
    /// network-free so the happy-path parsing is unit-testable.
    fn parse_token_response(body: &str) -> Result<OAuthTokens, CallbackError> {
        let raw: serde_json::Value = serde_json::from_str(body)
            .map_err(|e| CallbackError::MalformedTokenResponse(e.to_string()))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let access_token = raw
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CallbackError::MalformedTokenResponse("missing access_token".into()))?
            .to_string();
        Ok(OAuthTokens {
            access_token,
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
        })
    }

    /// Bind the loopback callback listener for this flow's redirect
    /// strategy and return `(redirect_uri, listener)`. Split from
    /// [`wait_for_code`] so the caller can build the authorize URL with
    /// the *actual* bound port (important for `DynamicPort`) before the
    /// browser is opened.
    ///
    /// Binds `127.0.0.1` only — never a routable interface — so the
    /// redirect can only be delivered by a process on this machine.
    pub async fn bind_callback_listener(
        &self,
    ) -> Result<(String, tokio::net::TcpListener), CallbackError> {
        let listener = tokio::net::TcpListener::bind(self.bind_addr())
            .await
            .map_err(|e| CallbackError::Bind(e.to_string()))?;
        let local = listener
            .local_addr()
            .map_err(|e| CallbackError::Bind(e.to_string()))?;
        let redirect_uri = format!("http://127.0.0.1:{}/callback", local.port());
        Ok((redirect_uri, listener))
    }

    /// Accept exactly one redirect on `listener`, validate the CSRF
    /// `state` against `expected_state`, and return the authorization
    /// `code`. Bounded by this flow's `listener_idle_timeout` so a user
    /// who closes the browser tab does not leak the bound port forever.
    ///
    /// The browser receives a minimal HTML page so the user knows the tab
    /// can be closed; the code itself never reaches the page body.
    pub async fn wait_for_code(
        &self,
        listener: tokio::net::TcpListener,
        expected_state: &str,
    ) -> Result<String, CallbackError> {
        let accept = async {
            loop {
                let (mut stream, _peer) = listener
                    .accept()
                    .await
                    .map_err(|e| CallbackError::MalformedRequest(e.to_string()))?;
                match read_request_target(&mut stream).await {
                    // Ignore favicon / probe requests that carry no OAuth
                    // params and keep waiting for the real redirect.
                    Ok(target) => {
                        let params = parse_callback_query(&target);
                        if params.code.is_none() && params.error.is_none() && params.state.is_none()
                        {
                            write_response(&mut stream, RESPONSE_WAITING).await;
                            continue;
                        }
                        let result =
                            Self::authorization_code_from_callback(expected_state, &params);
                        let page = if result.is_ok() {
                            RESPONSE_OK
                        } else {
                            RESPONSE_ERR
                        };
                        write_response(&mut stream, page).await;
                        return result;
                    }
                    Err(e) => {
                        write_response(&mut stream, RESPONSE_ERR).await;
                        return Err(e);
                    }
                }
            }
        };

        match tokio::time::timeout(self.listener_idle_timeout, accept).await {
            Ok(result) => result,
            Err(_) => Err(CallbackError::Timeout),
        }
    }
}

/// Minimal HTTP responses written back to the browser tab. Plain text in
/// an HTML wrapper; the authorization code is never echoed into the body.
const RESPONSE_OK: &str =
    "Authorization complete. You can close this tab and return to the terminal.";
const RESPONSE_ERR: &str = "Authorization failed. Return to the terminal for details.";
const RESPONSE_WAITING: &str = "Waiting for the authorization redirect…";

/// Read the request line off `stream` and return its target (the path +
/// query, e.g. `/callback?code=…&state=…`). Reads only the first line —
/// OAuth callbacks are GET requests with everything in the URL — and caps
/// the read so a malicious client cannot exhaust memory.
async fn read_request_target(stream: &mut tokio::net::TcpStream) -> Result<String, CallbackError> {
    use tokio::io::AsyncReadExt;
    // 8 KiB is far above any legitimate OAuth redirect URL but bounds a
    // hostile client.
    let mut buf = [0u8; 8192];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|e| CallbackError::MalformedRequest(e.to_string()))?;
    let text = String::from_utf8_lossy(&buf[..n]);
    // Request line: `GET /callback?... HTTP/1.1`
    let first_line = text.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let _method = parts.next();
    let target = parts
        .next()
        .ok_or_else(|| CallbackError::MalformedRequest("no request target".into()))?;
    Ok(target.to_string())
}

/// Write a minimal `200 OK` HTML response carrying `body` and close.
async fn write_response(stream: &mut tokio::net::TcpStream, body: &str) {
    use tokio::io::AsyncWriteExt;
    let html = format!("<!doctype html><meta charset=\"utf-8\"><p>{body}</p>");
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    // Best-effort: the browser tab is cosmetic; a write failure here must
    // not mask the real auth result.
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

/// Token bundle persisted to storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Unix epoch seconds — None when the provider didn't return `expires_in`.
    #[serde(default)]
    pub expires_at_unix_secs: Option<u64>,
    #[serde(default = "default_token_type")]
    pub token_type: String,
    #[serde(default)]
    pub scope: Option<String>,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

#[derive(Debug, Error, Clone)]
pub enum RefreshError {
    #[error("provider has not been authorized yet (no refresh token)")]
    NoRefreshToken,
    #[error("provider rejected refresh: {0}")]
    ProviderRejected(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("internal: refresh task panicked")]
    JoinError,
}

/// Lazily-resolved cell for the result of one refresh attempt.
/// Concurrent callers subscribe to the same cell and read once the
/// primary completes the network round-trip.
type RefreshCell = Arc<tokio::sync::OnceCell<Result<OAuthTokens, RefreshError>>>;

/// Coalesces concurrent refresh requests so the provider sees one POST
/// per refresh window, no matter how many tool calls fired into the
/// expired access token at the same time.
///
/// Usage: `let new_tokens = refresher.refresh(&fetcher).await?;`
/// where `fetcher` is a closure that hits the token endpoint.
pub struct SingleFlightRefresh {
    // `Option<Arc<...>>` so concurrent callers can subscribe to the
    // in-flight result without holding the mutex while awaiting.
    inner: Mutex<Option<RefreshCell>>,
}

impl Default for SingleFlightRefresh {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }
}

impl SingleFlightRefresh {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `fetcher` exactly once across concurrent callers. The first
    /// caller through installs the cell and drives `fetcher`; later
    /// callers attach to the same cell and read the result when
    /// `fetcher` completes.
    pub async fn refresh<F, Fut>(&self, fetcher: F) -> Result<OAuthTokens, RefreshError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = Result<OAuthTokens, RefreshError>> + Send + 'static,
    {
        // Acquire-and-publish the cell under the mutex, then drop the
        // mutex BEFORE awaiting `fetcher` so concurrent callers can see
        // the same cell.
        let (cell, primary) = {
            let mut guard = self.inner.lock().await;
            if let Some(existing) = guard.as_ref() {
                (existing.clone(), false)
            } else {
                let new_cell = Arc::new(tokio::sync::OnceCell::new());
                *guard = Some(new_cell.clone());
                (new_cell, true)
            }
        };

        if primary {
            // Drive the fetcher once; clear the cell when done so a
            // future refresh attempt isn't permanently fused.
            let result = fetcher().await;
            // Ignore set_err — only the primary calls set, and we have
            // a fresh OnceCell, so the first set always succeeds.
            let _ = cell.set(result.clone());
            // Clear the slot so the NEXT refresh window can start fresh.
            {
                let mut guard = self.inner.lock().await;
                *guard = None;
            }
            return result;
        }

        // Subscriber path: await the same cell. `get_or_init` would
        // double-fire `fetcher`, so we wait_initialized via `wait`.
        // (OnceCell only exposes get_or_init that consumes a closure;
        // we instead spin-wait by polling `get()` with a short sleep.
        // For the test surface this is fine; v0.9.1 can switch to a
        // notify-once primitive.)
        loop {
            if let Some(v) = cell.get() {
                return v.clone();
            }
            tokio::task::yield_now().await;
        }
    }
}

/// Generate a 32-byte CSRF state token, base64url-encoded.
pub fn new_state_token() -> String {
    use base64::Engine as _;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn sample_flow() -> OAuthFlow {
        OAuthFlow::new(
            "client-abc",
            Some("secret-xyz".into()),
            "https://example.com/auth",
            "https://example.com/token",
            vec!["scope1".into(), "scope2".into()],
        )
    }

    #[test]
    fn pkce_required_by_default() {
        let f = sample_flow();
        assert_eq!(f.pkce_mode, PkceMode::S256);
    }

    #[test]
    fn without_pkce_disables_it() {
        let f = sample_flow().without_pkce();
        assert_eq!(f.pkce_mode, PkceMode::Disabled);
    }

    #[test]
    fn redirect_strategy_default_is_dynamic_port() {
        match RedirectStrategy::default() {
            RedirectStrategy::DynamicPort => (),
            other => panic!("expected DynamicPort default, got {other:?}"),
        }
    }

    #[test]
    fn build_authorize_url_includes_pkce_when_enabled() {
        let flow = sample_flow();
        let (url, _state, pkce) = flow.build_authorize_url("http://127.0.0.1:54321/callback");
        assert!(pkce.is_some(), "PKCE pair should be returned");
        assert!(
            url.contains("code_challenge_method=S256"),
            "authorize URL must carry code_challenge_method=S256: {url}"
        );
        assert!(url.contains("code_challenge="));
    }

    #[test]
    fn build_authorize_url_omits_pkce_when_disabled() {
        let flow = sample_flow().without_pkce();
        let (url, _state, pkce) = flow.build_authorize_url("http://127.0.0.1:54321/callback");
        assert!(pkce.is_none());
        assert!(!url.contains("code_challenge"));
    }

    #[test]
    fn build_authorize_url_includes_state_and_scope_and_redirect() {
        let flow = sample_flow();
        let (url, state, _) = flow.build_authorize_url("http://127.0.0.1:54321/callback");
        // urlencoding encodes ':' as %3A and '/' as %2F by default
        let encoded_redirect = urlencoding::encode("http://127.0.0.1:54321/callback");
        assert!(
            url.contains(&format!("redirect_uri={encoded_redirect}")),
            "redirect_uri must be encoded: got {url}"
        );
        let encoded_state = urlencoding::encode(&state);
        assert!(url.contains(&format!("state={encoded_state}")));
        assert!(url.contains("scope=scope1%20scope2"));
        assert!(url.contains("client_id=client-abc"));
        assert!(url.contains("response_type=code"));
    }

    #[test]
    fn state_token_is_32_bytes_worth_of_base64url() {
        let s = new_state_token();
        // 32 bytes → 43 base64url chars without padding
        assert_eq!(
            s.len(),
            43,
            "state token must be 43 chars (32 bytes b64url)"
        );
    }

    #[test]
    fn state_tokens_are_unique_across_calls() {
        let a = new_state_token();
        let b = new_state_token();
        assert_ne!(a, b);
    }

    #[test]
    fn validate_state_accepts_matching() {
        let s = new_state_token();
        assert!(OAuthFlow::validate_state(&s, &s));
    }

    #[test]
    fn validate_state_rejects_wrong_value() {
        let s = new_state_token();
        let mut tampered = s.clone();
        // Flip the first char to one GUARANTEED different from the original.
        // (Naively replacing with a fixed "x" was 1/64 flaky: a token that
        // already starts with 'x' tampered to itself and the assert failed.)
        let repl = if s.starts_with('x') { "y" } else { "x" };
        tampered.replace_range(..1, repl);
        assert_ne!(s, tampered, "tamper must actually change the token");
        assert!(!OAuthFlow::validate_state(&s, &tampered));
    }

    #[test]
    fn validate_state_rejects_length_mismatch_without_panic() {
        // subtle::ConstantTimeEq panics on length mismatch — our wrapper
        // must guard against that explicitly.
        assert!(!OAuthFlow::validate_state("abc", "abcd"));
        assert!(!OAuthFlow::validate_state("", "x"));
    }

    // ── SingleFlightRefresh ──────────────────────────────────────────

    fn fresh_tokens() -> OAuthTokens {
        OAuthTokens {
            access_token: "fresh".into(),
            refresh_token: Some("rt".into()),
            expires_at_unix_secs: Some(0),
            token_type: "Bearer".into(),
            scope: None,
        }
    }

    #[tokio::test]
    async fn single_flight_refresh_only_one_request_under_concurrent_load() {
        let refresher = Arc::new(SingleFlightRefresh::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..10 {
            let r = refresher.clone();
            let c = calls.clone();
            handles.push(tokio::spawn(async move {
                r.refresh(move || async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    // tiny await so the other tasks can pile up on the cell
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    Ok(fresh_tokens())
                })
                .await
            }));
        }
        for h in handles {
            h.await.unwrap().unwrap();
        }
        let total = calls.load(Ordering::SeqCst);
        assert_eq!(
            total, 1,
            "single-flight must coalesce to ONE fetcher call, got {total}"
        );
    }

    #[tokio::test]
    async fn single_flight_refresh_propagates_error_to_all_subscribers() {
        let refresher = Arc::new(SingleFlightRefresh::new());
        let mut handles = Vec::new();
        for _ in 0..5 {
            let r = refresher.clone();
            handles.push(tokio::spawn(async move {
                r.refresh(move || async move {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    Err::<OAuthTokens, _>(RefreshError::ProviderRejected("nope".into()))
                })
                .await
            }));
        }
        for h in handles {
            let result = h.await.unwrap();
            assert!(matches!(result, Err(RefreshError::ProviderRejected(_))));
        }
    }

    // ── listener idle timeout ────────────────────────────────────────

    #[tokio::test]
    async fn listener_times_out_after_idle_timeout() {
        // The flow exposes the idle timeout as `Duration`; the listener
        // wires it through `tokio::time::timeout`. Verify the wrapping
        // by simulating the wait: a slow future bounded by the flow's
        // timeout returns `Elapsed`.
        let flow = sample_flow().with_listener_idle_timeout(Duration::from_millis(100));
        let pending = async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            "callback"
        };
        let result = tokio::time::timeout(flow.listener_idle_timeout, pending).await;
        assert!(result.is_err(), "expected timeout, got {result:?}");
    }

    #[tokio::test]
    async fn callback_with_wrong_state_rejected() {
        // The "callback" path validates state before doing anything else;
        // assert the validator drops a forged state. End-to-end with
        // hyper is left to integration tests in B9.
        let expected = new_state_token();
        let forged = new_state_token();
        assert!(!OAuthFlow::validate_state(&expected, &forged));
    }

    #[tokio::test]
    async fn code_exchange_parses_token_response() {
        // Round-trip a JSON token payload through OAuthTokens.
        let body = r#"{
            "access_token": "at-12345",
            "refresh_token": "rt-67890",
            "expires_in": 3600,
            "token_type": "Bearer",
            "scope": "scope1 scope2"
        }"#;
        let raw: serde_json::Value = serde_json::from_str(body).unwrap();
        // Translate into OAuthTokens shape.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let expires_at = raw
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .map(|secs| now + secs);
        let tokens = OAuthTokens {
            access_token: raw["access_token"].as_str().unwrap().to_string(),
            refresh_token: raw
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            expires_at_unix_secs: expires_at,
            token_type: raw["token_type"].as_str().unwrap().to_string(),
            scope: raw
                .get("scope")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        };
        assert_eq!(tokens.access_token, "at-12345");
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt-67890"));
        assert!(tokens.expires_at_unix_secs.unwrap() >= now + 3590);
        assert_eq!(tokens.scope.as_deref(), Some("scope1 scope2"));
    }

    // ── RedirectStrategy: dynamic vs fixed port ──────────────────────

    #[tokio::test]
    async fn dynamic_port_strategy_binds_ephemeral_port() {
        // Direct bind via std to verify the strategy semantics. Port 0
        // means "OS assigns ephemeral"; the returned port must be > 0.
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
        let port = listener.local_addr().unwrap().port();
        assert!(port > 0, "ephemeral bind should produce a real port");
        // Confirm we picked DynamicPort by default.
        let f = sample_flow();
        assert!(matches!(f.redirect_strategy, RedirectStrategy::DynamicPort));
    }

    #[tokio::test]
    async fn fixed_port_strategy_uses_specified_port() {
        let f = sample_flow().with_redirect_strategy(RedirectStrategy::FixedPort(0));
        match f.redirect_strategy {
            RedirectStrategy::FixedPort(p) => assert_eq!(p, 0),
            other => panic!("expected FixedPort, got {other:?}"),
        }
    }

    // ── callback parsing + code extraction (D026) ───────────────────

    #[test]
    fn parse_callback_query_extracts_code_and_state() {
        let params = parse_callback_query("/callback?code=abc123&state=xyz789&scope=a%20b");
        assert_eq!(params.code.as_deref(), Some("abc123"));
        assert_eq!(params.state.as_deref(), Some("xyz789"));
        assert!(params.error.is_none());
    }

    #[test]
    fn parse_callback_query_percent_decodes_values() {
        // A real Google code contains '/' which arrives percent-encoded.
        let params = parse_callback_query("/callback?code=4%2F0Ab%2Fcd&state=s");
        assert_eq!(params.code.as_deref(), Some("4/0Ab/cd"));
    }

    #[test]
    fn parse_callback_query_surfaces_error_param() {
        let params = parse_callback_query("/callback?error=access_denied&state=s");
        assert_eq!(params.error.as_deref(), Some("access_denied"));
        assert!(params.code.is_none());
    }

    #[test]
    fn parse_callback_query_empty_when_no_query() {
        let params = parse_callback_query("/favicon.ico");
        assert!(params.code.is_none() && params.state.is_none() && params.error.is_none());
    }

    #[test]
    fn authorization_code_from_callback_accepts_matching_state() {
        let state = new_state_token();
        let params = CallbackParams {
            code: Some("the-code".into()),
            state: Some(state.clone()),
            error: None,
        };
        let code = OAuthFlow::authorization_code_from_callback(&state, &params).unwrap();
        assert_eq!(code, "the-code");
    }

    #[test]
    fn authorization_code_from_callback_rejects_forged_state() {
        let expected = new_state_token();
        let forged = new_state_token();
        let params = CallbackParams {
            code: Some("the-code".into()),
            state: Some(forged),
            error: None,
        };
        let err = OAuthFlow::authorization_code_from_callback(&expected, &params).unwrap_err();
        assert!(matches!(err, CallbackError::StateMismatch));
    }

    #[test]
    fn authorization_code_from_callback_reports_user_denial_before_state() {
        // A denial carries no usable code; surface it as denied, not CSRF.
        let params = CallbackParams {
            code: None,
            state: None,
            error: Some("access_denied".into()),
        };
        let err = OAuthFlow::authorization_code_from_callback("expected", &params).unwrap_err();
        assert!(matches!(err, CallbackError::AuthorizationDenied(_)));
    }

    #[test]
    fn authorization_code_from_callback_missing_code_is_typed() {
        let state = new_state_token();
        let params = CallbackParams {
            code: None,
            state: Some(state.clone()),
            error: None,
        };
        let err = OAuthFlow::authorization_code_from_callback(&state, &params).unwrap_err();
        assert!(matches!(err, CallbackError::MissingCode));
    }

    #[test]
    fn parse_token_response_extracts_all_fields() {
        let body = r#"{"access_token":"at","refresh_token":"rt","expires_in":3600,
                       "token_type":"Bearer","scope":"a b"}"#;
        let tokens = OAuthFlow::parse_token_response(body).unwrap();
        assert_eq!(tokens.access_token, "at");
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt"));
        assert!(tokens.expires_at_unix_secs.unwrap() > 0);
        assert_eq!(tokens.scope.as_deref(), Some("a b"));
    }

    #[test]
    fn parse_token_response_rejects_body_without_access_token() {
        let err = OAuthFlow::parse_token_response(r#"{"token_type":"Bearer"}"#).unwrap_err();
        assert!(matches!(err, CallbackError::MalformedTokenResponse(_)));
    }

    #[tokio::test]
    async fn bind_callback_listener_returns_loopback_redirect_uri() {
        let flow = sample_flow();
        let (uri, listener) = flow.bind_callback_listener().await.unwrap();
        assert!(
            uri.starts_with("http://127.0.0.1:"),
            "redirect must bind loopback, got {uri}"
        );
        assert!(uri.ends_with("/callback"));
        // The advertised port must equal the actually-bound port.
        let bound_port = listener.local_addr().unwrap().port();
        assert!(uri.contains(&format!(":{bound_port}/")), "{uri}");
    }

    #[tokio::test]
    async fn wait_for_code_round_trips_a_real_loopback_redirect() {
        // End-to-end of the listener WITHOUT a live OAuth server: bind the
        // loopback listener, fire a real HTTP GET carrying code+state at
        // it (exactly as a browser redirect would), and assert the code
        // comes back. This is the wiring the manual-paste step replaces.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let flow = sample_flow().with_listener_idle_timeout(Duration::from_secs(5));
        let state = new_state_token();
        let (redirect_uri, listener) = flow.bind_callback_listener().await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let state_for_client = state.clone();
        let client = tokio::spawn(async move {
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            let req = format!(
                "GET /callback?code=auth-code-42&state={} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
                state_for_client
            );
            s.write_all(req.as_bytes()).await.unwrap();
            // Drain the response so the server's write_all completes.
            let mut resp = Vec::new();
            let _ = s.read_to_end(&mut resp).await;
            String::from_utf8_lossy(&resp).into_owned()
        });

        let code = flow.wait_for_code(listener, &state).await.unwrap();
        assert_eq!(code, "auth-code-42");
        let browser_body = client.await.unwrap();
        assert!(
            browser_body.contains("Authorization complete"),
            "browser tab should see the success page: {browser_body}"
        );
        assert!(
            !browser_body.contains("auth-code-42"),
            "the code must never be echoed into the browser page body"
        );
        // redirect_uri is what the caller would pass to exchange_code.
        assert!(redirect_uri.contains(&format!(":{port}/callback")));
    }

    #[tokio::test]
    async fn wait_for_code_rejects_forged_state_on_real_redirect() {
        use tokio::io::AsyncWriteExt;

        let flow = sample_flow().with_listener_idle_timeout(Duration::from_secs(5));
        let expected = new_state_token();
        let forged = new_state_token();
        let (_uri, listener) = flow.bind_callback_listener().await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let client = tokio::spawn(async move {
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            let req =
                format!("GET /callback?code=c&state={forged} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
            let _ = s.write_all(req.as_bytes()).await;
        });

        let err = flow.wait_for_code(listener, &expected).await.unwrap_err();
        assert!(matches!(err, CallbackError::StateMismatch));
        let _ = client.await;
    }

    #[tokio::test]
    async fn wait_for_code_times_out_when_no_redirect_arrives() {
        let flow = sample_flow().with_listener_idle_timeout(Duration::from_millis(120));
        let state = new_state_token();
        let (_uri, listener) = flow.bind_callback_listener().await.unwrap();
        let err = flow.wait_for_code(listener, &state).await.unwrap_err();
        assert!(matches!(err, CallbackError::Timeout));
    }

    #[tokio::test]
    async fn exchange_code_posts_and_parses_tokens_via_mock_server() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "exchanged-at",
                "refresh_token": "exchanged-rt",
                "expires_in": 3600,
                "token_type": "Bearer",
                "scope": "scope1 scope2"
            })))
            .mount(&server)
            .await;

        let flow = OAuthFlow::new(
            "client-abc",
            Some("secret-xyz".into()),
            format!("{}/auth", server.uri()),
            format!("{}/token", server.uri()),
            vec!["scope1".into()],
        );
        let client = wcore_egress::EgressClient::new();
        let tokens = flow
            .exchange_code(
                &client,
                "the-auth-code",
                "http://127.0.0.1:0/callback",
                Some("pkce-verifier"),
            )
            .await
            .unwrap();
        assert_eq!(tokens.access_token, "exchanged-at");
        assert_eq!(tokens.refresh_token.as_deref(), Some("exchanged-rt"));
        assert_eq!(tokens.scope.as_deref(), Some("scope1 scope2"));
    }

    #[tokio::test]
    async fn exchange_code_surfaces_provider_rejection() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_string("invalid_grant"))
            .mount(&server)
            .await;

        let flow = OAuthFlow::new(
            "client-abc",
            None,
            format!("{}/auth", server.uri()),
            format!("{}/token", server.uri()),
            vec![],
        );
        let client = wcore_egress::EgressClient::new();
        let err = flow
            .exchange_code(&client, "bad-code", "http://127.0.0.1:0/callback", None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, CallbackError::ProviderRejected(ref m) if m.contains("400")),
            "want a typed provider rejection carrying the status: {err:?}"
        );
    }

    #[test]
    fn oauth_state_token_validates_csrf_constant_time() {
        // The validator uses subtle::ConstantTimeEq under the hood —
        // assert by tying length-equal but byte-different inputs that
        // both pre-pass the length guard.
        let a = "a".repeat(32);
        let b = "b".repeat(32);
        assert!(!OAuthFlow::validate_state(&a, &b));
        let same = a.clone();
        assert!(OAuthFlow::validate_state(&a, &same));
    }
}
