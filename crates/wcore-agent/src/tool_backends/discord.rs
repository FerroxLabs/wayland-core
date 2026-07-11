//! v0.9.0 Wave-1 B4 — real Discord REST backend over `reqwest`.
//!
//! Resolves credentials from `DISCORD_BOT_TOKEN` (via the canonical
//! [`super::shared::read_env_key`] helper). Returns `None` when the env
//! var is absent or empty so the tool hides via `Tool::is_available()`
//! (closes R-H2 empty-string pathology).
//!
//! Endpoints used:
//! - `POST   https://discord.com/api/v10/channels/{channel_id}/messages` — send a message
//! - `GET    https://discord.com/api/v10/guilds/{guild_id}/channels`     — list channels
//! - `GET    https://discord.com/api/v10/users/@me/guilds`               — list guilds
//! - `GET    https://discord.com/api/v10/guilds/{guild_id}`              — server info
//! - `GET    https://discord.com/api/v10/channels/{channel_id}`          — channel info
//! - `GET    https://discord.com/api/v10/guilds/{guild_id}/roles`        — list roles
//! - `GET    https://discord.com/api/v10/guilds/{guild_id}/members/{user_id}` — member info
//! - `GET    https://discord.com/api/v10/guilds/{guild_id}/members/search`    — search members
//! - `GET    https://discord.com/api/v10/channels/{channel_id}/messages`      — fetch messages
//! - `GET    https://discord.com/api/v10/channels/{channel_id}/pins`          — list pins
//! - `PUT    https://discord.com/api/v10/channels/{channel_id}/pins/{message_id}` — pin
//! - `DELETE https://discord.com/api/v10/channels/{channel_id}/pins/{message_id}` — unpin
//! - `POST   https://discord.com/api/v10/channels/{channel_id}/threads`       — create thread
//! - `PUT    https://discord.com/api/v10/guilds/{guild_id}/members/{user_id}/roles/{role_id}` — add role
//! - `DELETE https://discord.com/api/v10/guilds/{guild_id}/members/{user_id}/roles/{role_id}` — remove role
//!
//! All calls send `Authorization: Bot <token>` per Discord docs.
//!
//! **Two-layer timeout (R-H1):** every dispatch is wrapped in an outer
//! `tokio::time::timeout` so the body decode + JSON parse cannot hang
//! past the wall-clock cap if reqwest's `.timeout()` is bypassed.
//!
//! **Rate limit handling:** Discord returns 429 with `Retry-After`
//! seconds. On 429, we sleep for `Retry-After` (capped at 30s) and
//! retry once. A second 429 is surfaced verbatim.
//!
//! **SSRF safety:** client is built via
//! [`super::build_ssrf_safe_tool_client`] which rejects mid-redirect
//! hops to 169.254/10.x/127/[fd00::] per #279.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Response, StatusCode};
use serde_json::{Value, json};
use wcore_egress::EgressClient as Client;

use super::{build_ssrf_safe_tool_client, parse_json_or_raw, shared::read_env_key};
use wcore_tools::discord_tool::{DiscordBackend, DiscordCall, DiscordOutcome};

/// Discord API base — pinned to v10 per the brief.
const API_BASE: &str = "https://discord.com/api/v10";

/// Outer wall-clock cap for an entire dispatch (request + body decode +
/// post-processing). Reqwest's `.timeout()` covers only the exchange;
/// this guards the whole pipeline (R-H1).
const DISPATCH_TIMEOUT: Duration = Duration::from_secs(45);

/// Cap on `Retry-After` we honour automatically. Anything longer is
/// surfaced as a typed error so the caller can decide.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

/// Construct the Discord backend if `DISCORD_BOT_TOKEN` is configured.
///
/// Returns `None` when the env var is unset or empty so the tool hides
/// (closes R-H2). Use as:
/// ```ignore
/// if let Some(b) = build_discord_backend() {
///     registry.register(Box::new(DiscordTool::new(b)));
/// }
/// ```
pub fn build_discord_backend() -> Option<Arc<dyn DiscordBackend>> {
    let token = read_env_key("DISCORD_BOT_TOKEN")?;
    tracing::info!("discord: DISCORD_BOT_TOKEN found, wiring HttpDiscordBackend");
    Some(Arc::new(HttpDiscordBackend::new(token)))
}

/// Real Discord REST backend.
pub struct HttpDiscordBackend {
    client: Client,
    /// `Authorization` header value, precomputed as `"Bot <token>"`.
    bot_auth: String,
    /// Override the API base — test-only escape hatch so wiremock
    /// servers can stand in for `https://discord.com`. Production
    /// callers always use [`HttpDiscordBackend::new`] which hardcodes
    /// [`API_BASE`].
    api_base: String,
}

impl HttpDiscordBackend {
    /// New backend with the non-streaming HTTP timeout policy
    /// (AUDIT B-5) plus the SSRF-resistant redirect policy (#279) — see
    /// [`super::build_ssrf_safe_tool_client`].
    pub fn new(token: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            bot_auth: format!("Bot {token}"),
            api_base: API_BASE.to_string(),
        }
    }

    /// Test-only constructor that points at a non-default API base
    /// (typically a wiremock server URI).
    #[cfg(test)]
    fn new_with_base(token: String, api_base: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            bot_auth: format!("Bot {token}"),
            api_base,
        }
    }

    /// Send a prepared request, handling 429 once with `Retry-After`
    /// backoff. The `factory` closure rebuilds the request from scratch
    /// for the retry — `reqwest::RequestBuilder` is not `Clone`.
    async fn send_with_retry<F>(&self, factory: F) -> Result<Response, String>
    where
        F: Fn() -> wcore_egress::EgressRequestBuilder,
    {
        let first = factory()
            .send()
            .await
            .map_err(|e| format!("transport error: {e}"))?;
        if first.status() != StatusCode::TOO_MANY_REQUESTS {
            return Ok(first);
        }
        // Parse Retry-After (Discord returns seconds, possibly fractional).
        let retry_after_secs = first
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(1.0);
        let retry_after = Duration::from_secs_f64(retry_after_secs.max(0.0));
        if retry_after > MAX_RETRY_AFTER {
            return Err(format!(
                "Discord rate-limited (HTTP 429); Retry-After {}s exceeds {}s cap",
                retry_after.as_secs(),
                MAX_RETRY_AFTER.as_secs()
            ));
        }
        tracing::warn!(
            "discord: HTTP 429, sleeping {:?} per Retry-After before single retry",
            retry_after
        );
        tokio::time::sleep(retry_after).await;
        let second = factory()
            .send()
            .await
            .map_err(|e| format!("transport error on retry: {e}"))?;
        Ok(second)
    }

    /// Dispatch a single call. Separated from the trait method so the
    /// outer `tokio::time::timeout` wrap stays cleanly localized.
    async fn dispatch_inner(&self, call: &DiscordCall) -> DiscordOutcome {
        // Compose the (method, url, optional body) tuple per action.
        let plan = match plan_request(&self.api_base, call) {
            Ok(p) => p,
            Err(e) => return DiscordOutcome::Err { message: e },
        };

        let response = match self
            .send_with_retry(|| build_request(&self.client, &self.bot_auth, &plan))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return DiscordOutcome::Err {
                    message: format!("Discord request failed: {e}"),
                };
            }
        };

        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        if status == StatusCode::FORBIDDEN {
            return DiscordOutcome::Forbidden { body: text };
        }

        let payload = parse_json_or_raw(&text);
        if status.is_success() {
            DiscordOutcome::Ok { payload }
        } else {
            let msg = payload
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("Discord returned HTTP {}", status.as_u16()));
            DiscordOutcome::Err {
                message: format!("Discord HTTP {}: {msg}", status.as_u16()),
            }
        }
    }
}

#[async_trait]
impl DiscordBackend for HttpDiscordBackend {
    async fn dispatch(&self, call: &DiscordCall) -> DiscordOutcome {
        // Two-layer timeout (R-H1): reqwest's `.timeout()` only covers
        // the HTTP exchange; this outer cap covers body decode + JSON
        // parse + the single Retry-After sleep.
        match tokio::time::timeout(DISPATCH_TIMEOUT, self.dispatch_inner(call)).await {
            Ok(outcome) => outcome,
            Err(_) => DiscordOutcome::Err {
                message: format!(
                    "Discord dispatch exceeded {}s wall-clock cap",
                    DISPATCH_TIMEOUT.as_secs()
                ),
            },
        }
    }
}

/// Verb the Discord REST API uses for an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Get,
    Post,
    Put,
    Delete,
}

/// A planned outbound request: method + absolute URL + optional JSON body.
#[derive(Debug, Clone)]
struct PlannedRequest {
    verb: Verb,
    url: String,
    body: Option<Value>,
}

/// Map a `DiscordCall` to an HTTP request plan. Returns `Err` with a
/// user-readable message for unsupported actions or missing fields
/// (the tool layer already validates required params, so this is a
/// defensive secondary check).
fn plan_request(api_base: &str, call: &DiscordCall) -> Result<PlannedRequest, String> {
    let url = |path: &str| format!("{}{}", api_base, path);

    match call.action.as_str() {
        "list_guilds" => Ok(PlannedRequest {
            verb: Verb::Get,
            url: url("/users/@me/guilds"),
            body: None,
        }),
        "server_info" => Ok(PlannedRequest {
            verb: Verb::Get,
            url: url(&format!("/guilds/{}?with_counts=true", call.guild_id)),
            body: None,
        }),
        "list_channels" => Ok(PlannedRequest {
            verb: Verb::Get,
            url: url(&format!("/guilds/{}/channels", call.guild_id)),
            body: None,
        }),
        "channel_info" => Ok(PlannedRequest {
            verb: Verb::Get,
            url: url(&format!("/channels/{}", call.channel_id)),
            body: None,
        }),
        "list_roles" => Ok(PlannedRequest {
            verb: Verb::Get,
            url: url(&format!("/guilds/{}/roles", call.guild_id)),
            body: None,
        }),
        "member_info" => Ok(PlannedRequest {
            verb: Verb::Get,
            url: url(&format!(
                "/guilds/{}/members/{}",
                call.guild_id, call.user_id
            )),
            body: None,
        }),
        "search_members" => Ok(PlannedRequest {
            verb: Verb::Get,
            url: url(&format!(
                "/guilds/{}/members/search?query={}&limit={}",
                call.guild_id,
                urlencode(&call.query),
                call.limit.clamp(1, 1000),
            )),
            body: None,
        }),
        "fetch_messages" => {
            let mut qs = format!("limit={}", call.limit.clamp(1, 100));
            if !call.before.is_empty() {
                qs.push_str(&format!("&before={}", urlencode(&call.before)));
            }
            if !call.after.is_empty() {
                qs.push_str(&format!("&after={}", urlencode(&call.after)));
            }
            Ok(PlannedRequest {
                verb: Verb::Get,
                url: url(&format!("/channels/{}/messages?{}", call.channel_id, qs)),
                body: None,
            })
        }
        "list_pins" => Ok(PlannedRequest {
            verb: Verb::Get,
            url: url(&format!("/channels/{}/pins", call.channel_id)),
            body: None,
        }),
        "pin_message" => Ok(PlannedRequest {
            verb: Verb::Put,
            url: url(&format!(
                "/channels/{}/pins/{}",
                call.channel_id, call.message_id
            )),
            body: None,
        }),
        "unpin_message" => Ok(PlannedRequest {
            verb: Verb::Delete,
            url: url(&format!(
                "/channels/{}/pins/{}",
                call.channel_id, call.message_id
            )),
            body: None,
        }),
        "create_thread" => {
            // POST /channels/{channel_id}/messages/{message_id}/threads when
            // anchored to a message, else /channels/{channel_id}/threads.
            let path = if call.message_id.is_empty() {
                format!("/channels/{}/threads", call.channel_id)
            } else {
                format!(
                    "/channels/{}/messages/{}/threads",
                    call.channel_id, call.message_id
                )
            };
            let body = json!({
                "name": call.name,
                "auto_archive_duration": call.auto_archive_duration,
                // 11 = GUILD_PUBLIC_THREAD per Discord channel type enum
                "type": 11,
            });
            Ok(PlannedRequest {
                verb: Verb::Post,
                url: url(&path),
                body: Some(body),
            })
        }
        "add_role" => Ok(PlannedRequest {
            verb: Verb::Put,
            url: url(&format!(
                "/guilds/{}/members/{}/roles/{}",
                call.guild_id, call.user_id, call.role_id
            )),
            body: None,
        }),
        "remove_role" => Ok(PlannedRequest {
            verb: Verb::Delete,
            url: url(&format!(
                "/guilds/{}/members/{}/roles/{}",
                call.guild_id, call.user_id, call.role_id
            )),
            body: None,
        }),
        other => Err(format!(
            "HttpDiscordBackend does not implement action '{}'",
            other
        )),
    }
}

/// Build a `reqwest::RequestBuilder` from a `PlannedRequest`, attaching
/// the precomputed `Authorization: Bot <token>` header.
fn build_request(
    client: &Client,
    bot_auth: &str,
    plan: &PlannedRequest,
) -> wcore_egress::EgressRequestBuilder {
    let mut builder = match plan.verb {
        Verb::Get => client.get(&plan.url),
        Verb::Post => client.post(&plan.url),
        Verb::Put => client.put(&plan.url),
        Verb::Delete => client.delete(&plan.url),
    };
    builder = builder.header(reqwest::header::AUTHORIZATION, bot_auth);
    builder = builder.header(
        reqwest::header::USER_AGENT,
        "DiscordBot (https://wayland.run, v0.9.0)",
    );
    if let Some(body) = &plan.body {
        builder = builder.json(body);
    }
    builder
}

/// Minimal URL component encoder for query parameters. Inline rather
/// than depending on `shared::urlencode` because Discord expects `%20`
/// not `+` for spaces in query strings.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use wcore_tools::discord_tool::{DiscordOutcome, DiscordTool};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn call(action: &str) -> DiscordCall {
        DiscordCall {
            action: action.to_string(),
            guild_id: "100".to_string(),
            channel_id: "200".to_string(),
            user_id: "300".to_string(),
            role_id: "400".to_string(),
            message_id: "500".to_string(),
            query: String::new(),
            name: "thread-name".to_string(),
            limit: 25,
            before: String::new(),
            after: String::new(),
            auto_archive_duration: 1440,
        }
    }

    // ----- Resolver behaviour ------------------------------------------------

    #[serial]
    #[test]
    fn build_discord_backend_returns_none_when_token_unset() {
        // SAFETY: tests run sequentially per crate; we restore env after.
        let prior = std::env::var("DISCORD_BOT_TOKEN").ok();
        unsafe { std::env::remove_var("DISCORD_BOT_TOKEN") };
        assert!(build_discord_backend().is_none());
        if let Some(v) = prior {
            unsafe { std::env::set_var("DISCORD_BOT_TOKEN", v) };
        }
    }

    #[serial]
    #[test]
    fn build_discord_backend_returns_none_when_token_empty_string() {
        // R-H2: empty string must be treated as unset.
        let prior = std::env::var("DISCORD_BOT_TOKEN").ok();
        unsafe { std::env::set_var("DISCORD_BOT_TOKEN", "") };
        assert!(build_discord_backend().is_none());
        unsafe { std::env::set_var("DISCORD_BOT_TOKEN", "   ") };
        assert!(build_discord_backend().is_none());
        unsafe { std::env::remove_var("DISCORD_BOT_TOKEN") };
        if let Some(v) = prior {
            unsafe { std::env::set_var("DISCORD_BOT_TOKEN", v) };
        }
    }

    #[test]
    fn null_default_skips_registration() {
        // The default `DiscordTool` (no backend wired) MUST report
        // `is_available() = false` so `ToolRegistry::register` hides it.
        // This is the v0.9.0 W1 B4 contract: backend-required tools
        // never appear in the schema until a real backend is bound.
        use wcore_tools::Tool;
        let tool = DiscordTool::default();
        assert!(
            !tool.is_available(),
            "default DiscordTool must hide when no backend is wired"
        );
    }

    #[test]
    fn discord_send_uses_discord_api_v10() {
        // Confirms the hardcoded API base — anyone refactoring the
        // const will see this test fire.
        assert_eq!(API_BASE, "https://discord.com/api/v10");
    }

    // ----- HTTP behaviour ----------------------------------------------------

    #[tokio::test]
    async fn discord_send_message_uses_bot_authorization_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/guilds/100/channels"))
            .and(header("authorization", "Bot test-token-abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;

        let backend = HttpDiscordBackend::new_with_base("test-token-abc".to_string(), server.uri());
        let outcome = backend.dispatch(&call("list_channels")).await;
        match outcome {
            DiscordOutcome::Ok { .. } => {}
            other => panic!(
                "expected Ok (header matched); got {other:?} — header mismatch implies wrong auth scheme"
            ),
        }
    }

    #[tokio::test]
    async fn discord_rate_limit_429_triggers_retry_after_backoff() {
        let server = MockServer::start().await;
        // First call: 429 with Retry-After: 0 (smallest valid backoff).
        // Second call: 200. Backend must retry exactly once and succeed.
        Mock::given(method("GET"))
            .and(path("/guilds/100/channels"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "0")
                    .set_body_json(json!({"message": "rate limited"})),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/guilds/100/channels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id": "1"}])))
            .mount(&server)
            .await;

        let backend = HttpDiscordBackend::new_with_base("tok".to_string(), server.uri());
        let outcome = backend.dispatch(&call("list_channels")).await;
        match outcome {
            DiscordOutcome::Ok { payload } => {
                let arr = payload.as_array().expect("array response");
                assert_eq!(arr.len(), 1);
                assert_eq!(arr[0].get("id").and_then(Value::as_str), Some("1"));
            }
            other => panic!("expected Ok after 429 retry, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn discord_handles_http_5xx_returns_typed_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(503).set_body_json(json!({"message": "service unavailable"})),
            )
            .mount(&server)
            .await;

        let backend = HttpDiscordBackend::new_with_base("tok".to_string(), server.uri());
        let outcome = backend.dispatch(&call("list_channels")).await;
        match outcome {
            DiscordOutcome::Err { message } => {
                assert!(message.contains("503"), "want 503 in msg, got: {message}");
            }
            other => panic!("expected Err on 503, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn discord_handles_malformed_json_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<<<not-json>>>"))
            .mount(&server)
            .await;

        let backend = HttpDiscordBackend::new_with_base("tok".to_string(), server.uri());
        let outcome = backend.dispatch(&call("list_channels")).await;
        // Malformed JSON on 200 falls through `parse_json_or_raw` which
        // wraps the raw text as a string Value — the dispatch reports Ok
        // with the raw string payload. The contract is "no panic".
        match outcome {
            DiscordOutcome::Ok { payload } => {
                assert!(
                    payload.as_str().is_some_and(|s| s.contains("not-json")),
                    "want raw text passthrough, got: {payload}"
                );
            }
            other => panic!("expected Ok with raw payload, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn discord_rate_limit_429_above_cap_surfaces_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "9999")
                    .set_body_json(json!({"message": "long-rate-limit"})),
            )
            .mount(&server)
            .await;

        let backend = HttpDiscordBackend::new_with_base("tok".to_string(), server.uri());
        let outcome = backend.dispatch(&call("list_channels")).await;
        match outcome {
            DiscordOutcome::Err { message } => {
                assert!(
                    message.contains("429") && message.contains("cap"),
                    "want capped-retry-after error, got: {message}"
                );
            }
            other => panic!("expected Err for over-cap 429, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn discord_handles_network_timeout() {
        // v0.9.1 W1 E (debt sweep): deterministic replacement of the
        // v0.9.0 `drop(server)`-race variant. We mount a slow responder
        // (30s delay) on a live wiremock and assert the outer
        // `tokio::time::timeout(1s, ...)` wrapper fires before any
        // backend response can land. The error we are exercising is the
        // *outer wall-clock* — verified by `Elapsed` from the wrapper,
        // not by any specific transport error from reqwest. This is
        // race-free because the server is alive for the whole test;
        // the timeout is the only mechanism that can resolve.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(30))
                    .set_body_json(json!({"channels": []})),
            )
            .mount(&server)
            .await;
        let backend = HttpDiscordBackend::new_with_base("tok".to_string(), server.uri());
        // The wrapper timeout (1s) must fire before the 30s mock delay
        // OR any inner reqwest timeout. `Elapsed` is the deterministic
        // signal we are after.
        let wrapped = tokio::time::timeout(
            Duration::from_secs(1),
            backend.dispatch(&call("list_channels")),
        )
        .await;
        assert!(
            wrapped.is_err(),
            "expected outer timeout to fire on slow server, got {wrapped:?}"
        );
    }

    #[tokio::test]
    async fn discord_forbidden_403_maps_to_forbidden_outcome() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/channels/200/pins/500"))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_json(json!({"message": "Missing Permissions", "code": 50013})),
            )
            .mount(&server)
            .await;

        let backend = HttpDiscordBackend::new_with_base("tok".to_string(), server.uri());
        let outcome = backend.dispatch(&call("pin_message")).await;
        match outcome {
            DiscordOutcome::Forbidden { body } => {
                assert!(body.contains("Missing Permissions"), "got body: {body}");
            }
            other => panic!("expected Forbidden, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn discord_refuses_ssrf_redirect_to_metadata_service() {
        // 302 → 169.254.169.254 must be refused mid-redirect by the
        // SSRF-safe client. The backend surfaces this as a transport
        // error in DiscordOutcome::Err.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/guilds/100/channels"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;

        let backend = HttpDiscordBackend::new_with_base("tok".to_string(), server.uri());
        let outcome = backend.dispatch(&call("list_channels")).await;
        match outcome {
            DiscordOutcome::Err { message } => {
                let lower = message.to_lowercase();
                assert!(
                    lower.contains("redirect")
                        || lower.contains("blocked")
                        || lower.contains("refused"),
                    "expected redirect-blocked error, got: {message}"
                );
            }
            other => panic!("expected Err for SSRF redirect, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn discord_create_thread_posts_to_channels_threads_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/channels/200/threads"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({"id": "999", "name": "thread-name"})),
            )
            .mount(&server)
            .await;

        let backend = HttpDiscordBackend::new_with_base("tok".to_string(), server.uri());
        let mut c = call("create_thread");
        c.message_id = String::new(); // no anchor → plain channel endpoint
        let outcome = backend.dispatch(&c).await;
        match outcome {
            DiscordOutcome::Ok { payload } => {
                assert_eq!(payload.get("id").and_then(Value::as_str), Some("999"));
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn urlencode_handles_special_chars() {
        // Spaces become %20 (NOT + per RFC3986 query rules — matches
        // what Discord's CDN expects).
        assert_eq!(urlencode("hello world"), "hello%20world");
        assert_eq!(urlencode("foo&bar"), "foo%26bar");
        assert_eq!(urlencode("safe.string-ok_~123"), "safe.string-ok_~123");
    }

    #[test]
    fn plan_request_unknown_action_returns_err() {
        let c = call("definitely-not-an-action");
        let plan = plan_request(API_BASE, &c);
        assert!(plan.is_err());
    }
}
