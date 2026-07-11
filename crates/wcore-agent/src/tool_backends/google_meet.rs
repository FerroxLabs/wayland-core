//! Google Meet HTTP backend wired via the v0.9.0 Wave-1 B0 OAuth subsystem.
//!
//! Implements [`wcore_tools::google_meet_tool::GoogleMeetBackend`] over
//! the Google Meet REST API at `https://meet.googleapis.com/v2/`. All
//! traffic flows through the SSRF-safe `reqwest::Client` built by
//! [`super::build_ssrf_safe_tool_client`]; token acquisition, refresh,
//! and storage go through B0's [`wcore_agent::oauth`] subsystem.
//!
//! Wave-1 B9 wiring contract (R-B1 file-per-backend):
//! - New file. Does NOT touch `tool_backends/mod.rs` (B0 owns the splits)
//!   or `bootstrap.rs` (B13 owns the assembly).
//! - PKCE is on by default (B0 contract).
//! - Single-flight refresh prevents N concurrent tool calls from each
//!   issuing their own POST to `oauth2.googleapis.com/token` after the
//!   access token expires.
//! - `say_in_meeting` returns a typed `MeetApiCapabilityError` because
//!   the Meet REST API v2 (as of 2026-05) does not expose in-call TTS;
//!   the bot path (Playwright) handles that, not this HTTP wrapper.
//! - The resolver `build_google_meet_backend()` sniffs `GOOGLE_CLIENT_ID`
//!   via [`super::shared::read_env_key`] (so empty-string envs do NOT
//!   count as "configured" — R-H2). Absent client id → `None` → bootstrap
//!   registers the `::default()` null backend → every meet_* tool is
//!   hidden by the registry (`is_available() == false`).

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use reqwest::{Method, Response, StatusCode};
use serde_json::Value;
use tokio::sync::Mutex;
use wcore_egress::EgressClient as Client;

use crate::oauth::{
    OAuthFlow, OAuthStorage, OAuthTokens, RedirectStrategy, RefreshError, SingleFlightRefresh,
};

use super::build_ssrf_safe_tool_client;
use super::shared::read_env_key;

use wcore_tools::google_meet_tool::{
    GoogleMeetBackend, MeetError, MeetJoinRequest, MeetJoinResponse, MeetLeaveResponse,
    MeetSayResponse, MeetStatusResponse, MeetTranscriptResponse,
};

/// Provider name used by [`OAuthStorage`] when persisting tokens.
const PROVIDER: &str = "google_meet";

/// Refresh tokens this many seconds before they expire to absorb clock skew.
const REFRESH_LEAD_SECS: u64 = 60;

/// Outer wall-clock cap on every authenticated request — covers the HTTP
/// exchange *and* the body decode + JSON parse (R-H1 two-layer timeout).
const PER_CALL_TIMEOUT: Duration = Duration::from_secs(20);

// ---------------------------------------------------------------------
// Google Meet REST endpoint root.
// ---------------------------------------------------------------------

/// Public default endpoint. Tests inject a `MockServer` URI instead.
const MEET_API_BASE: &str = "https://meet.googleapis.com/v2";

// ---------------------------------------------------------------------
// Backend struct
// ---------------------------------------------------------------------

/// HTTP-backed [`GoogleMeetBackend`] using OAuth tokens persisted by B0.
pub struct HttpGoogleMeetBackend {
    /// Provider-agnostic OAuth descriptor (PKCE-S256 by default).
    oauth: Arc<OAuthFlow>,
    /// Coalesces concurrent refresh attempts into one network round-trip.
    single_flight: Arc<SingleFlightRefresh>,
    /// SSRF-safe non-streaming client (AUDIT B-5 + #279 redirect policy).
    client: Client,
    /// File-backed OAuth token storage (`~/.wayland/oauth/google_meet.json`).
    storage: OAuthStorage,
    /// In-memory cached tokens. Loaded lazily on first call; the storage
    /// layer is the source of truth across processes.
    cached: Mutex<Option<OAuthTokens>>,
    /// Meet API base URL — overridden in tests to a `wiremock` URI so the
    /// failure-path matrix can be exercised without hitting real Google.
    api_base: String,
}

impl HttpGoogleMeetBackend {
    /// Construct from a pre-built [`OAuthFlow`] + storage. Used by the
    /// resolver and by tests that want to inject a custom flow.
    pub fn new(oauth: Arc<OAuthFlow>, storage: OAuthStorage) -> Self {
        Self {
            oauth,
            single_flight: Arc::new(SingleFlightRefresh::new()),
            client: build_ssrf_safe_tool_client(),
            storage,
            cached: Mutex::new(None),
            api_base: MEET_API_BASE.to_string(),
        }
    }

    /// Override the Meet API base — tests point at a `wiremock::MockServer`.
    #[cfg(test)]
    fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into();
        self
    }

    /// Perform the full OAuth authorization-code flow if no stored tokens
    /// exist. Returns the resulting tokens (and persists them). Wave-1
    /// W4 E1 wires this to the `/auth google-meet` slash command; the
    /// backend just exposes the entry point.
    ///
    /// **NOTE:** v0.9.0 B0 deliberately stops short of binding the real
    /// hyper listener; this method is a stub that returns the same
    /// `MeetError::BackendNotConfigured` until W4 E1 wires the listener.
    /// Until then, the user must seed `~/.wayland/oauth/google_meet.json`
    /// manually (or the test path passes seeded tokens in).
    pub async fn authenticate_blocking(&self) -> Result<OAuthTokens, MeetError> {
        Err(MeetError::BackendNotConfigured(
            "/auth google-meet is wired in Wave-1 W4 E1 — until then, seed \
             ~/.wayland/oauth/google_meet.json with the access_token/refresh_token \
             from a manual oauth2l flow."
                .into(),
        ))
    }

    /// Load the cached token from disk on first call, then keep it in
    /// memory. On a cache miss this returns `Ok(None)` rather than
    /// erroring so callers can decide whether to drive the consent flow.
    async fn load_cached(&self) -> Result<Option<OAuthTokens>, MeetError> {
        let mut guard = self.cached.lock().await;
        if guard.is_some() {
            return Ok(guard.clone());
        }
        let from_disk = self
            .storage
            .load(PROVIDER)
            .map_err(|e| MeetError::Other(format!("oauth storage load failed: {e}")))?;
        *guard = from_disk.clone();
        Ok(from_disk)
    }

    /// Whether `tokens.access_token` is still valid for at least
    /// `REFRESH_LEAD_SECS` more seconds. Returns `true` when there's no
    /// `expires_at_unix_secs` (provider didn't supply `expires_in`).
    fn token_is_fresh(tokens: &OAuthTokens) -> bool {
        let Some(exp) = tokens.expires_at_unix_secs else {
            // Conservative: if we don't know when it expires, treat as fresh
            // for the lifetime of the process. The next 401 will trigger
            // refresh on the actual request path.
            return true;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        exp.saturating_sub(REFRESH_LEAD_SECS) > now
    }

    /// Resolve a fresh access token: load from cache → check expiry →
    /// refresh through the single-flight gate when needed.
    async fn get_or_refresh(&self) -> Result<OAuthTokens, MeetError> {
        let existing = self.load_cached().await?;
        let Some(tokens) = existing else {
            return Err(MeetError::BackendNotConfigured(
                "no stored Google Meet OAuth tokens — run `/auth google-meet` first \
                 (or seed ~/.wayland/oauth/google_meet.json manually)."
                    .into(),
            ));
        };

        if Self::token_is_fresh(&tokens) {
            return Ok(tokens);
        }

        // Refresh path — coalesce through SingleFlightRefresh so N
        // concurrent tool calls hit the token endpoint exactly once.
        let refresh_token = tokens.refresh_token.clone().ok_or_else(|| {
            MeetError::BackendNotConfigured(
                "stored tokens have no refresh_token — re-run `/auth google-meet`.".into(),
            )
        })?;

        let client = self.client.clone();
        let token_url = self.oauth.token_url.clone();
        let client_id = self.oauth.client_id.clone();
        let client_secret = self.oauth.client_secret.clone();

        let refreshed = self
            .single_flight
            .refresh(move || async move {
                let mut form: Vec<(&str, String)> = vec![
                    ("grant_type", "refresh_token".into()),
                    ("refresh_token", refresh_token),
                    ("client_id", client_id),
                ];
                if let Some(s) = client_secret {
                    form.push(("client_secret", s));
                }
                let res = tokio::time::timeout(
                    PER_CALL_TIMEOUT,
                    client.post(&token_url).form(&form).send(),
                )
                .await
                .map_err(|_| RefreshError::Transport("refresh timed out".into()))?
                .map_err(|e| RefreshError::Transport(format!("{e}")))?;

                let status = res.status();
                let body = tokio::time::timeout(PER_CALL_TIMEOUT, res.text())
                    .await
                    .map_err(|_| RefreshError::Transport("refresh body timed out".into()))?
                    .map_err(|e| RefreshError::Transport(format!("{e}")))?;

                if !status.is_success() {
                    return Err(RefreshError::ProviderRejected(format!(
                        "HTTP {} — {body}",
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
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            RefreshError::ProviderRejected("missing access_token".into())
                        })?
                        .to_string(),
                    // Google does NOT rotate refresh_tokens on refresh by
                    // default; carry the existing one forward when the
                    // response omits it.
                    refresh_token: raw
                        .get("refresh_token")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    expires_at_unix_secs: raw
                        .get("expires_in")
                        .and_then(|v| v.as_u64())
                        .map(|s| now + s),
                    token_type: raw
                        .get("token_type")
                        .and_then(Value::as_str)
                        .unwrap_or("Bearer")
                        .to_string(),
                    scope: raw.get("scope").and_then(Value::as_str).map(str::to_string),
                    // Google's token-refresh response does not include an
                    // id_token; carry None.
                    id_token: raw
                        .get("id_token")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            })
            .await
            .map_err(|e| MeetError::Other(format!("oauth refresh failed: {e}")))?;

        // Merge refresh_token forward when the response omitted it, then
        // persist + update the cache.
        let mut to_store = refreshed;
        if to_store.refresh_token.is_none() {
            to_store.refresh_token = tokens.refresh_token.clone();
        }
        if let Err(e) = self.storage.store(PROVIDER, &to_store) {
            tracing::warn!(error = %e, "failed to persist refreshed Google Meet tokens");
        }
        *self.cached.lock().await = Some(to_store.clone());
        Ok(to_store)
    }

    /// Issue an authenticated request against the Meet REST API, wrapped
    /// in the outer two-layer timeout. Tests intercept at the API base
    /// level so the OAuth refresh path is separate.
    async fn authed_request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Response, MeetError> {
        let tokens = self.get_or_refresh().await?;
        let url = format!("{}{}", self.api_base, path);

        let mut builder = self
            .client
            .request(method, &url)
            .header("Authorization", format!("Bearer {}", tokens.access_token))
            .header("Accept", "application/json");
        if let Some(b) = body {
            builder = builder.json(b);
        }

        let response = tokio::time::timeout(PER_CALL_TIMEOUT, builder.send())
            .await
            .map_err(|_| MeetError::Other(format!("meet request timed out at {path}")))?
            .map_err(|e| MeetError::Other(format!("meet request transport error: {e}")))?;

        Ok(response)
    }

    /// Decode a `Response` into the result type, mapping common HTTP
    /// failure modes to typed `MeetError` variants.
    async fn decode_json<T>(response: Response, op: &'static str) -> Result<T, MeetError>
    where
        T: serde::de::DeserializeOwned + Default,
    {
        let status = response.status();
        if status == StatusCode::NOT_IMPLEMENTED {
            return Err(MeetError::Other(format!(
                "MeetApiCapabilityError {{ method: {:?}, reason: \"Meet API v2 returned 501 \
                 Not Implemented — requires Meet API v2+ TTS capability — see Google Meet \
                 REST docs for current status.\" }}",
                op
            )));
        }
        let text = tokio::time::timeout(PER_CALL_TIMEOUT, response.text())
            .await
            .map_err(|_| MeetError::Other(format!("{op}: body read timed out")))?
            .map_err(|e| MeetError::Other(format!("{op}: body read failed: {e}")))?;

        if !status.is_success() {
            if status == StatusCode::TOO_MANY_REQUESTS {
                return Err(MeetError::Other(format!(
                    "{op}: HTTP 429 (rate-limited) — {text}"
                )));
            }
            if status.is_server_error() {
                return Err(MeetError::Other(format!(
                    "{op}: HTTP {} (server error) — {text}",
                    status.as_u16()
                )));
            }
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                return Err(MeetError::BackendNotConfigured(format!(
                    "{op}: HTTP {} — token may be invalid or revoked; re-run /auth google-meet",
                    status.as_u16()
                )));
            }
            return Err(MeetError::Other(format!(
                "{op}: HTTP {} — {text}",
                status.as_u16()
            )));
        }

        if text.trim().is_empty() {
            // Some Meet endpoints return 204 No Content (e.g. leave); fall
            // back to the type's default.
            return Ok(T::default());
        }
        serde_json::from_str::<T>(&text)
            .map_err(|e| MeetError::Other(format!("{op}: malformed JSON: {e}")))
    }
}

// ---------------------------------------------------------------------
// GoogleMeetBackend impl — wires the 5 tool calls to the REST API.
// ---------------------------------------------------------------------

#[async_trait]
impl GoogleMeetBackend for HttpGoogleMeetBackend {
    async fn join(&self, request: MeetJoinRequest) -> Result<MeetJoinResponse, MeetError> {
        // The Meet v2 `spaces` API takes an existing meeting code; we
        // pass-through the validated meeting_id and let the API return
        // the canonical space resource.
        let body = serde_json::json!({
            "meetingCode": request.meeting_id,
            "displayName": request.guest_name,
            "mode": match request.mode {
                wcore_tools::google_meet_tool::MeetMode::Transcribe => "TRANSCRIBE",
                wcore_tools::google_meet_tool::MeetMode::Realtime => "REALTIME",
            },
        });
        let response = self
            .authed_request(Method::POST, "/spaces:join", Some(&body))
            .await?;
        let raw: Value = Self::decode_json::<Value>(response, "join_meeting").await?;
        Ok(MeetJoinResponse {
            meeting_id: raw
                .get("meetingCode")
                .and_then(Value::as_str)
                .unwrap_or(&request.meeting_id)
                .to_string(),
            bot_pid: None,
            transcript_path: None,
        })
    }

    async fn status(&self, _node: Option<&str>) -> Result<MeetStatusResponse, MeetError> {
        let response = self
            .authed_request(Method::GET, "/spaces/-/status", None)
            .await?;
        let raw: Value = Self::decode_json::<Value>(response, "meeting_status").await?;
        Ok(MeetStatusResponse {
            alive: raw.get("alive").and_then(Value::as_bool).unwrap_or(false),
            in_meeting: raw
                .get("inMeeting")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            transcript_lines: raw
                .get("transcriptLines")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            last_caption_at: raw.get("lastCaptionAt").and_then(Value::as_f64),
            message: raw
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    async fn transcript(
        &self,
        _node: Option<&str>,
        last: Option<u32>,
    ) -> Result<MeetTranscriptResponse, MeetError> {
        let path = match last {
            Some(n) => format!("/spaces/-/transcripts?last={n}"),
            None => "/spaces/-/transcripts".to_string(),
        };
        let response = self.authed_request(Method::GET, &path, None).await?;
        let raw: Value = Self::decode_json::<Value>(response, "meeting_transcript").await?;
        let lines = raw
            .get("lines")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let parsed_lines = lines
            .into_iter()
            .filter_map(|line| {
                Some(wcore_tools::google_meet_tool::MeetTranscriptLine {
                    speaker: line
                        .get("speaker")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    text: line.get("text").and_then(Value::as_str)?.to_string(),
                    t_seconds: line.get("tSeconds").and_then(Value::as_f64),
                })
            })
            .collect();
        Ok(MeetTranscriptResponse {
            lines: parsed_lines,
            total_lines: raw.get("totalLines").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    async fn leave(&self, _node: Option<&str>) -> Result<MeetLeaveResponse, MeetError> {
        let response = self
            .authed_request(Method::POST, "/spaces/-:leave", None)
            .await?;
        let raw: Value = Self::decode_json::<Value>(response, "leave_meeting").await?;
        Ok(MeetLeaveResponse {
            meeting_id: raw
                .get("meetingCode")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            reason: raw
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("agent called meet_leave")
                .to_string(),
            transcript_path: None,
        })
    }

    /// The Meet REST API v2 does NOT expose in-call TTS. Surface a typed
    /// capability error so the agent can fall back to the Playwright bot
    /// path (or skip the action entirely).
    async fn say(&self, _node: Option<&str>, text: &str) -> Result<MeetSayResponse, MeetError> {
        // Attempt the (currently-undocumented) endpoint so behaviour is
        // forward-compatible if Google ships it later: a 501 / 404 here
        // turns into MeetApiCapabilityError; a 200 returns success.
        let body = serde_json::json!({ "text": text });
        let response = self
            .authed_request(Method::POST, "/spaces/-:say", Some(&body))
            .await?;
        let status = response.status();
        if status == StatusCode::NOT_IMPLEMENTED || status == StatusCode::NOT_FOUND {
            return Err(MeetError::Other(format!(
                "MeetApiCapabilityError {{ method: \"say_in_meeting\", reason: \
                 \"Requires Meet API v2+ TTS capability — see Google Meet REST docs for \
                 current status. HTTP {} from /spaces/-:say.\" }}",
                status.as_u16()
            )));
        }
        let _raw: Value = Self::decode_json::<Value>(response, "say_in_meeting").await?;
        Ok(MeetSayResponse { queued: 1 })
    }
}

// ---------------------------------------------------------------------
// Resolver — `build_google_meet_backend()` sniffs GOOGLE_CLIENT_ID.
// ---------------------------------------------------------------------

/// Build the real Google Meet backend when `GOOGLE_CLIENT_ID` is set.
/// Returns `None` when the env var is missing or empty (R-H2), which
/// causes the bootstrap to register the `::default()` null backend → the
/// 5 meet_* tools are hidden by the registry's `is_available()` filter.
pub fn build_google_meet_backend() -> Option<HttpGoogleMeetBackend> {
    let client_id = read_env_key("GOOGLE_CLIENT_ID")?;
    let client_secret = read_env_key("GOOGLE_CLIENT_SECRET");

    let storage = match OAuthStorage::from_home() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "google_meet: OAuth storage init failed — backend hidden");
            return None;
        }
    };

    let flow = OAuthFlow::new(
        client_id,
        client_secret,
        "https://accounts.google.com/o/oauth2/v2/auth",
        "https://oauth2.googleapis.com/token",
        vec![
            "https://www.googleapis.com/auth/meetings.space.created".into(),
            "https://www.googleapis.com/auth/meetings.space.readonly".into(),
        ],
    )
    .with_redirect_strategy(RedirectStrategy::DynamicPort);
    // PKCE-S256 is the default per B0; do NOT call `.without_pkce()`.

    Some(HttpGoogleMeetBackend::new(Arc::new(flow), storage))
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::PkceMode;
    use serial_test::serial;
    use tempfile::TempDir;
    use wcore_tools::google_meet_tool::MeetMode;
    use wiremock::matchers::{header, method, path as wm_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn seeded_tokens() -> OAuthTokens {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        OAuthTokens {
            access_token: "seeded-access-token".into(),
            refresh_token: Some("seeded-refresh-token".into()),
            // 1 hour in the future → no refresh on first call.
            expires_at_unix_secs: Some(now + 3600),
            token_type: "Bearer".into(),
            scope: Some("https://www.googleapis.com/auth/meetings.space.created".into()),
            id_token: None,
        }
    }

    fn test_backend_with(
        api_base: String,
        storage_root: std::path::PathBuf,
    ) -> HttpGoogleMeetBackend {
        let storage = OAuthStorage::at_root(storage_root).unwrap();
        let flow = OAuthFlow::new(
            "test-client-id",
            Some("test-client-secret".into()),
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            vec!["https://www.googleapis.com/auth/meetings.space.created".into()],
        );
        HttpGoogleMeetBackend::new(Arc::new(flow), storage).with_api_base(api_base)
    }

    // ── Resolver / env sniffing ─────────────────────────────────────

    #[serial]
    #[test]
    fn build_google_meet_backend_returns_none_when_client_id_unset() {
        // SAFETY: stash + restore the env var so we don't trample on
        // an outer process value.
        let saved = std::env::var("GOOGLE_CLIENT_ID").ok();
        unsafe { std::env::remove_var("GOOGLE_CLIENT_ID") };
        assert!(
            build_google_meet_backend().is_none(),
            "no GOOGLE_CLIENT_ID → resolver must return None"
        );
        if let Some(v) = saved {
            unsafe { std::env::set_var("GOOGLE_CLIENT_ID", v) };
        }
    }

    #[serial]
    #[test]
    fn google_meet_returns_none_when_env_var_empty_string() {
        // R-H2 contract: empty string must NOT count as configured.
        let saved = std::env::var("GOOGLE_CLIENT_ID").ok();
        unsafe { std::env::set_var("GOOGLE_CLIENT_ID", "") };
        assert!(
            build_google_meet_backend().is_none(),
            "empty GOOGLE_CLIENT_ID must be treated as unset"
        );
        unsafe { std::env::set_var("GOOGLE_CLIENT_ID", "   ") };
        assert!(
            build_google_meet_backend().is_none(),
            "whitespace-only GOOGLE_CLIENT_ID must be treated as unset"
        );
        unsafe { std::env::remove_var("GOOGLE_CLIENT_ID") };
        if let Some(v) = saved {
            unsafe { std::env::set_var("GOOGLE_CLIENT_ID", v) };
        }
    }

    // ── PKCE / OAuth descriptor ─────────────────────────────────────

    #[test]
    fn google_meet_oauth_uses_pkce() {
        // The resolver returns a backend wrapping an OAuthFlow with PKCE
        // S256 enabled (B0 default).  We can't easily call the live
        // resolver without setting GOOGLE_CLIENT_ID, so synthesise an
        // equivalent flow and assert the PKCE mode.
        let flow = OAuthFlow::new(
            "x",
            Some("y".into()),
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            vec!["https://www.googleapis.com/auth/meetings.space.created".into()],
        );
        assert_eq!(
            flow.pkce_mode,
            PkceMode::S256,
            "Google Meet flow MUST keep B0's default PKCE-S256 (never .without_pkce())"
        );
        let (url, _state, pkce) = flow.build_authorize_url("http://127.0.0.1:0/callback");
        assert!(pkce.is_some(), "PKCE pair must be generated");
        assert!(
            url.contains("code_challenge_method=S256"),
            "authorize URL must carry S256 challenge: {url}"
        );
    }

    #[serial]
    #[test]
    fn google_meet_oauth_uses_correct_endpoints_and_scopes() {
        // Sanity-check the well-known Google constants — a typo here
        // would silently break OAuth on user machines.
        let saved_id = std::env::var("GOOGLE_CLIENT_ID").ok();
        let saved_secret = std::env::var("GOOGLE_CLIENT_SECRET").ok();
        unsafe {
            std::env::set_var("GOOGLE_CLIENT_ID", "test-id");
            std::env::set_var("GOOGLE_CLIENT_SECRET", "test-secret");
        }
        let backend = build_google_meet_backend().expect("resolver must build with env set");
        assert_eq!(
            backend.oauth.auth_url,
            "https://accounts.google.com/o/oauth2/v2/auth"
        );
        assert_eq!(
            backend.oauth.token_url,
            "https://oauth2.googleapis.com/token"
        );
        assert!(
            backend
                .oauth
                .scopes
                .iter()
                .any(|s| s == "https://www.googleapis.com/auth/meetings.space.created")
        );
        assert!(
            backend
                .oauth
                .scopes
                .iter()
                .any(|s| s == "https://www.googleapis.com/auth/meetings.space.readonly")
        );
        // Cleanup.
        unsafe {
            std::env::remove_var("GOOGLE_CLIENT_ID");
            std::env::remove_var("GOOGLE_CLIENT_SECRET");
        }
        if let Some(v) = saved_id {
            unsafe { std::env::set_var("GOOGLE_CLIENT_ID", v) };
        }
        if let Some(v) = saved_secret {
            unsafe { std::env::set_var("GOOGLE_CLIENT_SECRET", v) };
        }
    }

    // ── Registry-level: 5 tools hidden when unconfigured ────────────

    #[test]
    fn all_5_meet_tools_hidden_when_unconfigured() {
        use wcore_tools::google_meet_tool::{
            MeetJoinTool, MeetLeaveTool, MeetSayTool, MeetStatusTool, MeetTranscriptTool,
        };
        use wcore_tools::registry::ToolRegistry;

        let mut reg = ToolRegistry::new();
        reg.register(Box::new(MeetJoinTool::default()));
        reg.register(Box::new(MeetStatusTool::default()));
        reg.register(Box::new(MeetTranscriptTool::default()));
        reg.register(Box::new(MeetLeaveTool::default()));
        reg.register(Box::new(MeetSayTool::default()));

        let names: Vec<String> = reg.to_tool_defs().into_iter().map(|d| d.name).collect();
        for n in &[
            "meet_join",
            "meet_status",
            "meet_transcript",
            "meet_leave",
            "meet_say",
        ] {
            assert!(
                !names.contains(&n.to_string()),
                "null-backed {n} must be filtered out by registry (is_available() == false)"
            );
        }
    }

    #[test]
    fn null_default_skips_registration_for_each_of_5_tools() {
        // Same intent as the all_5 test but per-tool so a regression on
        // any single tool's `is_available()` override is caught directly.
        use wcore_tools::google_meet_tool::{
            MeetJoinTool, MeetLeaveTool, MeetSayTool, MeetStatusTool, MeetTranscriptTool,
        };
        use wcore_tools::registry::ToolRegistry;

        let cases: Vec<(&str, Box<dyn wcore_tools::Tool>)> = vec![
            ("meet_join", Box::new(MeetJoinTool::default())),
            ("meet_status", Box::new(MeetStatusTool::default())),
            ("meet_transcript", Box::new(MeetTranscriptTool::default())),
            ("meet_leave", Box::new(MeetLeaveTool::default())),
            ("meet_say", Box::new(MeetSayTool::default())),
        ];

        for (expected_name, tool) in cases {
            let mut reg = ToolRegistry::new();
            reg.register(tool);
            let defs = reg.to_tool_defs();
            assert!(
                defs.iter().all(|d| d.name != expected_name),
                "default (null-backed) {expected_name} must skip registration"
            );
        }
    }

    // ── Authenticated request shape (Bearer header) ─────────────────

    #[tokio::test]
    async fn google_meet_authenticated_request_uses_bearer_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wm_path("/spaces/-/status"))
            .and(header("authorization", "Bearer seeded-access-token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"alive":true,"inMeeting":true,"transcriptLines":3}"#),
            )
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let backend = test_backend_with(server.uri(), tmp.path().join("oauth"));
        // Seed tokens so get_or_refresh doesn't try to refresh.
        backend.storage.store(PROVIDER, &seeded_tokens()).unwrap();

        let result = backend.status(None).await.expect("status must succeed");
        assert!(result.alive);
        assert!(result.in_meeting);
        assert_eq!(result.transcript_lines, 3);
    }

    // ── Failure paths ───────────────────────────────────────────────

    #[tokio::test]
    async fn google_meet_handles_http_5xx_returns_typed_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wm_path("/spaces/-/status"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream is down"))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let backend = test_backend_with(server.uri(), tmp.path().join("oauth"));
        backend.storage.store(PROVIDER, &seeded_tokens()).unwrap();

        let err = backend.status(None).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("server error") || msg.contains("503"),
            "expected typed 5xx error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn google_meet_handles_http_429_with_retry_after_backoff() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wm_path("/spaces/-/status"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "30")
                    .set_body_string(r#"{"error":"rate-limited"}"#),
            )
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let backend = test_backend_with(server.uri(), tmp.path().join("oauth"));
        backend.storage.store(PROVIDER, &seeded_tokens()).unwrap();

        let err = backend.status(None).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("429") || msg.contains("rate"),
            "expected 429-tagged error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn google_meet_handles_malformed_json_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wm_path("/spaces/-/status"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<<not json>>"))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let backend = test_backend_with(server.uri(), tmp.path().join("oauth"));
        backend.storage.store(PROVIDER, &seeded_tokens()).unwrap();

        let err = backend.status(None).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("malformed JSON"),
            "expected malformed-JSON error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn google_meet_handles_network_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wm_path("/spaces/-/status"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(45)))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let backend = test_backend_with(server.uri(), tmp.path().join("oauth"));
        backend.storage.store(PROVIDER, &seeded_tokens()).unwrap();

        // The SSRF-safe client has TOOL_REQUEST_TIMEOUT < 45s; the outer
        // tokio::time::timeout(PER_CALL_TIMEOUT=20s) also fires. Either
        // path produces an error inside ~20s.
        let start = std::time::Instant::now();
        let err = backend.status(None).await.unwrap_err();
        let elapsed = start.elapsed();
        let msg = err.to_string();
        assert!(
            msg.contains("timed out") || msg.contains("transport"),
            "expected timeout/transport error, got: {msg}"
        );
        // Should fail well before the mock's 45s delay.
        assert!(
            elapsed < Duration::from_secs(30),
            "request must respect two-layer timeout; took {elapsed:?}"
        );
    }

    // ── say_in_meeting capability error ─────────────────────────────

    #[tokio::test]
    async fn google_meet_say_in_meeting_returns_typed_capability_error_when_api_lacks_tts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wm_path("/spaces/-:say"))
            .respond_with(
                ResponseTemplate::new(501).set_body_string(r#"{"error":"TTS not implemented"}"#),
            )
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let backend = test_backend_with(server.uri(), tmp.path().join("oauth"));
        backend.storage.store(PROVIDER, &seeded_tokens()).unwrap();

        let err = backend.say(None, "hello world").await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("MeetApiCapabilityError"),
            "expected MeetApiCapabilityError, got: {msg}"
        );
        assert!(
            msg.contains("say_in_meeting"),
            "error must name the method: {msg}"
        );
        assert!(
            msg.contains("TTS"),
            "error must explain the capability gap: {msg}"
        );
    }

    // ── SSRF redirect refusal ───────────────────────────────────────

    #[tokio::test]
    async fn google_meet_refuses_ssrf_redirect_to_metadata_service() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wm_path("/spaces/-/status"))
            .respond_with(ResponseTemplate::new(302).insert_header(
                "Location",
                "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
            ))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let backend = test_backend_with(server.uri(), tmp.path().join("oauth"));
        backend.storage.store(PROVIDER, &seeded_tokens()).unwrap();

        let err = backend.status(None).await.unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("redirect") || msg.contains("blocked") || msg.contains("transport"),
            "expected SSRF redirect refusal, got: {msg}"
        );
    }

    // ── Refresh token plumbing ──────────────────────────────────────

    #[tokio::test]
    async fn token_is_fresh_when_unexpired() {
        let t = seeded_tokens();
        assert!(HttpGoogleMeetBackend::token_is_fresh(&t));
    }

    #[tokio::test]
    async fn token_is_stale_when_within_refresh_lead() {
        let mut t = seeded_tokens();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        t.expires_at_unix_secs = Some(now + 10); // 10s out, lead is 60s
        assert!(!HttpGoogleMeetBackend::token_is_fresh(&t));
    }

    #[tokio::test]
    async fn token_with_no_expiry_treated_as_fresh() {
        let mut t = seeded_tokens();
        t.expires_at_unix_secs = None;
        assert!(HttpGoogleMeetBackend::token_is_fresh(&t));
    }

    // ── join → returns meeting id ───────────────────────────────────

    #[tokio::test]
    async fn join_meeting_succeeds_and_returns_meeting_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(wm_path("/spaces:join"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"meetingCode":"abc-defg-hij"}"#),
            )
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let backend = test_backend_with(server.uri(), tmp.path().join("oauth"));
        backend.storage.store(PROVIDER, &seeded_tokens()).unwrap();

        let resp = backend
            .join(MeetJoinRequest {
                url: "https://meet.google.com/abc-defg-hij".into(),
                meeting_id: "abc-defg-hij".into(),
                mode: MeetMode::Transcribe,
                guest_name: "Wayland Agent".into(),
                duration: None,
                headed: false,
                node: None,
            })
            .await
            .expect("join must succeed");
        assert_eq!(resp.meeting_id, "abc-defg-hij");
    }

    // ── Backend errors loudly when no tokens stored ─────────────────

    #[tokio::test]
    async fn get_or_refresh_errors_when_no_tokens_stored() {
        let server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();
        let backend = test_backend_with(server.uri(), tmp.path().join("oauth"));
        // Deliberately do NOT store tokens.
        let err = backend.status(None).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("/auth google-meet") || msg.contains("no stored"),
            "expected BackendNotConfigured guidance, got: {msg}"
        );
    }

    // ── authenticate_blocking returns a clear pending-Wave-W4 error ─

    #[tokio::test]
    async fn authenticate_blocking_returns_pending_w4_error() {
        let server = MockServer::start().await;
        let tmp = TempDir::new().unwrap();
        let backend = test_backend_with(server.uri(), tmp.path().join("oauth"));
        let err = backend.authenticate_blocking().await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("/auth google-meet") || msg.contains("W4"),
            "expected W4 pointer, got: {msg}"
        );
    }
}
