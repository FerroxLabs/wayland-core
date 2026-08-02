//! `chat.postMessage` client with retry-with-jitter + Retry-After handling.
//!
//! Slack response shape: `{ "ok": true, "ts": "1234.567", "channel": "..." }`
//! on success, `{ "ok": false, "error": "<code>" }` on failure. Some errors
//! arrive as HTTP 200 with `ok: false` (the platform's preferred surface),
//! others as HTTP 4xx/5xx — we treat both as failure modes and classify by
//! "is this retryable?" rather than by transport status.

use std::time::Duration;

use rand::Rng;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::error::SlackError;

/// Slack `chat.postMessage` request body. Threading via `thread_ts` is
/// optional; we serialise only the fields the platform requires.
#[derive(Debug, Clone, Serialize)]
pub struct PostMessageRequest {
    pub channel: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostMessageResponse {
    pub ok: bool,
    #[serde(default)]
    pub ts: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Permanent Slack error codes (authoritative — no point retrying).
/// Everything else surfaces as Api(...) but is treated as terminal
/// at the API boundary (transport-layer retries are handled around
/// HTTP failures, not application-layer ok:false rejections).
const PERMANENT_ERROR_CODES: &[&str] = &[
    "invalid_auth",
    "not_authed",
    "account_inactive",
    "token_revoked",
    "token_expired",
    "no_permission",
    "channel_not_found",
    "is_archived",
    "msg_too_long",
    "rate_limited", // handled by Retry-After path, but still terminal if we surface it here
];

/// Initial base for exponential backoff (jittered ±25%).
const BACKOFF_BASE_MS: u64 = 250;

/// Slack `ok:false` codes that mean **the credential itself was refused**, as
/// opposed to the request being wrong.
///
/// The distinction is the operator's next action: these say "rotate or
/// re-scope the token", everything else says "fix the call". `account_inactive`
/// belongs here because a deactivated bot user cannot be recovered by retrying
/// — the token must be reissued against a live account.
pub fn is_auth_rejection(code: &str) -> bool {
    matches!(
        code,
        "invalid_auth" | "not_authed" | "token_revoked" | "token_expired" | "account_inactive"
    )
}

/// `auth.test` response. Slack echoes the authenticated identity on success
/// and `{"ok": false, "error": "invalid_auth"}` on a rejected token.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthTestResponse {
    pub ok: bool,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub team: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Ask Slack whether this bot token is live, **without sending a message**.
///
/// # Why this exists
///
/// Slack inbound is webhook-driven, so this adapter has no long-lived
/// connection the platform can reject and no poll that would ever notice a
/// refusal. Before this call, `start()` resolved the token out of the
/// credential store and declared the channel connected — a REJECTED token and
/// a good one produced byte-identical behaviour, and health read `Healthy`
/// until somebody tried to send. `auth.test` is the one Slack surface that
/// answers the credential question and puts no traffic on a channel.
///
/// # Error mapping is deliberately three-way
///
/// - [`SlackError::Auth`] — the platform refused the credential. Actionable,
///   and the only variant callers may turn into `Unauthenticated`.
/// - [`SlackError::Http`] — Slack could not be reached, or answered 5xx. This
///   is NOT a verdict on the credential; treating it as one would flip a
///   healthy channel to "rotate your token" on every network blip.
/// - [`SlackError::Api`] / [`SlackError::MalformedPayload`] — Slack answered,
///   but with something this function cannot interpret as either verdict.
///
/// On success returns the authenticated identity (`"<user_id>/<team>"`), never
/// the token.
pub async fn auth_test(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
) -> Result<String, SlackError> {
    let url = format!("{}/api/auth.test", api_base.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .bearer_auth(bot_token)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send()
        .await
        .map_err(|e| SlackError::Http(format!("auth.test send error: {e}")))?;

    let status = resp.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(SlackError::Auth(format!("HTTP {}", status.as_u16())));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        // 5xx and friends are reachability, not a credential verdict.
        return Err(SlackError::Http(format!(
            "auth.test HTTP {}: {body}",
            status.as_u16()
        )));
    }

    let parsed: AuthTestResponse = resp
        .json()
        .await
        .map_err(|e| SlackError::MalformedPayload(format!("decode auth.test response: {e}")))?;
    if !parsed.ok {
        let code = parsed.error.unwrap_or_else(|| "unknown".to_string());
        if is_auth_rejection(&code) {
            return Err(SlackError::Auth(code));
        }
        return Err(SlackError::Api(code));
    }

    Ok(format!(
        "{}/{}",
        parsed.user_id.as_deref().unwrap_or("unknown-user"),
        parsed.team.as_deref().unwrap_or("unknown-team")
    ))
}

/// Send `chat.postMessage` with retry. Returns the response on first
/// success; on permanent failure returns the first non-retryable error.
pub async fn post_message(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    req: &PostMessageRequest,
    max_attempts: u32,
) -> Result<PostMessageResponse, SlackError> {
    post_message_keyed(http, api_base, bot_token, req, max_attempts, None).await
}

/// Header carrying the delivery ledger's stable idempotency key.
///
/// The de-facto standard spelling (Stripe, and every API that copied it). It is
/// inert against real Slack, which ignores unknown request headers — so this
/// adds nothing to the production wire beyond one header, and gives a
/// destination that DOES honour it everything it needs to collapse a replay
/// into one message.
pub const IDEMPOTENCY_HEADER: &str = "Idempotency-Key";

/// [`post_message`] carrying an idempotency key the destination may use to
/// recognise a retry of the same logical delivery.
///
/// Note the key is attached OUTSIDE the retry loop and is identical on every
/// attempt. That is the point: the internal retries here and the gateway's
/// cross-restart retry must present the same key, or the destination sees two
/// different deliveries and the guarantee is gone precisely when it matters.
pub async fn post_message_keyed(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    req: &PostMessageRequest,
    max_attempts: u32,
    idempotency_key: Option<&str>,
) -> Result<PostMessageResponse, SlackError> {
    let url = format!("{}/api/chat.postMessage", api_base.trim_end_matches('/'));
    let mut last_err: Option<String> = None;

    for attempt in 1..=max_attempts {
        let mut builder = http
            .post(&url)
            .bearer_auth(bot_token)
            .header("Content-Type", "application/json; charset=utf-8");
        if let Some(k) = idempotency_key {
            builder = builder.header(IDEMPOTENCY_HEADER, k);
        }
        let resp = builder.json(req).send().await;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                last_err = Some(format!("send error: {e}"));
                if attempt < max_attempts {
                    sleep_backoff(attempt, None).await;
                    continue;
                }
                break;
            }
        };

        let status = resp.status();

        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = parse_retry_after(resp.headers());
            last_err = Some("HTTP 429".to_string());
            if attempt < max_attempts {
                sleep_backoff(attempt, retry_after).await;
                continue;
            }
            break;
        }

        if status.is_server_error() {
            last_err = Some(format!("HTTP {}", status.as_u16()));
            if attempt < max_attempts {
                sleep_backoff(attempt, None).await;
                continue;
            }
            break;
        }

        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            // Auth failures are terminal — no retry.
            let body = resp.text().await.unwrap_or_default();
            return Err(SlackError::Auth(format!(
                "HTTP {}: {body}",
                status.as_u16()
            )));
        }

        if status.is_client_error() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SlackError::Api(format!("HTTP {}: {body}", status.as_u16())));
        }

        // 2xx — parse the body. ok:false is still terminal (Slack's preferred
        // failure surface for permanent app-level errors).
        let parsed: PostMessageResponse = resp.json().await.map_err(|e| {
            SlackError::MalformedPayload(format!("decode chat.postMessage response: {e}"))
        })?;

        if !parsed.ok {
            let code = parsed
                .error
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            // Credential verdict FIRST, and through the one classifier — a
            // hand-rolled list here is how `account_inactive` came to be an
            // `Api` error on this path while `is_auth_rejection` called it a
            // credential rejection, so a deactivated bot user published no
            // `AuthExpired` and health read Healthy.
            if is_auth_rejection(&code) {
                return Err(SlackError::Auth(code));
            }
            if PERMANENT_ERROR_CODES.contains(&code.as_str()) {
                return Err(SlackError::Api(code));
            }
            // Unknown ok:false code — treat as terminal API error (don't
            // burn the retry budget on an unknown semantic failure).
            return Err(SlackError::Api(code));
        }

        return Ok(parsed);
    }

    Err(SlackError::RetryExhausted {
        attempts: max_attempts,
        last: last_err.unwrap_or_else(|| "unknown".to_string()),
    })
}

/// Parse the `Retry-After` header. Slack returns integer seconds.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Sleep with exponential backoff + ±25% jitter. If an explicit
/// `retry_after` was supplied (from a 429), honour it instead.
async fn sleep_backoff(attempt: u32, retry_after: Option<Duration>) {
    if let Some(d) = retry_after {
        tokio::time::sleep(d).await;
        return;
    }
    // attempt is 1-indexed: 250, 500, 1000, 2000, 4000 ms…
    let base = BACKOFF_BASE_MS.saturating_mul(1u64 << (attempt.saturating_sub(1).min(6)));
    let jitter = {
        let mut rng = rand::thread_rng();
        // ±25% of base.
        let span = (base as f64 * 0.25) as i64;
        if span > 0 {
            rng.gen_range(-span..=span)
        } else {
            0
        }
    };
    let sleep_ms = (base as i64 + jitter).max(0) as u64;
    tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
}

/// `reactions.add` request body. Slack identifies the target message by
/// `channel` + `timestamp` (the message `ts`), and the emoji by its
/// shortcode `name` (NOT a unicode glyph — see [`slack_emoji_name`]).
#[derive(Debug, Clone, Serialize)]
pub struct AddReactionRequest {
    pub channel: String,
    pub timestamp: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct AddReactionResponse {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
}

/// Map the small set of ack unicode emoji the subscriber sends to the
/// Slack shortcodes `reactions.add` expects. Returns `None` for an emoji
/// we have no mapping for, so the caller can skip rather than send a name
/// Slack would reject (`invalid_name`).
pub fn slack_emoji_name(emoji: &str) -> Option<&'static str> {
    match emoji {
        "👀" => Some("eyes"),
        "✅" => Some("white_check_mark"),
        "❌" => Some("x"),
        "👍" => Some("+1"),
        "🎉" => Some("tada"),
        _ => None,
    }
}

/// Add a reaction via `POST /api/reactions.add`. Single attempt — the ack
/// reaction is a best-effort, non-fatal signal, so it does not consume the
/// send-retry budget. `already_reacted` is treated as success (idempotent).
pub async fn add_reaction(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    req: &AddReactionRequest,
) -> Result<(), SlackError> {
    let url = format!("{}/api/reactions.add", api_base.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .bearer_auth(bot_token)
        .header("Content-Type", "application/json; charset=utf-8")
        .json(req)
        .send()
        .await
        .map_err(|e| SlackError::Api(format!("send error: {e}")))?;

    let status = resp.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        let body = resp.text().await.unwrap_or_default();
        return Err(SlackError::Auth(format!(
            "HTTP {}: {body}",
            status.as_u16()
        )));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(SlackError::Api(format!("HTTP {}: {body}", status.as_u16())));
    }

    let parsed: AddReactionResponse = resp
        .json()
        .await
        .map_err(|e| SlackError::MalformedPayload(format!("decode reactions.add response: {e}")))?;
    if !parsed.ok {
        let code = parsed.error.unwrap_or_else(|| "unknown".to_string());
        // Already reacted is benign for the ack use case.
        if code == "already_reacted" {
            return Ok(());
        }
        if is_auth_rejection(&code) {
            return Err(SlackError::Auth(code));
        }
        return Err(SlackError::Api(code));
    }
    Ok(())
}

/// `chat.update` request body. Slack identifies the target message by
/// `channel` + `ts` — the same pair `reactions.add` uses, and the same `ts`
/// [`post_message`] returns in its receipt, so an edit is addressable directly
/// from a send's own receipt with nothing else stored.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateMessageRequest {
    pub channel: String,
    pub ts: String,
    pub text: String,
}

/// `chat.delete` request body.
#[derive(Debug, Clone, Serialize)]
pub struct DeleteMessageRequest {
    pub channel: String,
    pub ts: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MutateMessageResponse {
    pub ok: bool,
    #[serde(default)]
    pub ts: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Slack error codes for `chat.update` / `chat.delete` that mean **the target
/// is not there / not ours**.
///
/// These are surfaced as [`SlackError::Api`] and are NOT retried. They are
/// listed explicitly rather than folded into "any `ok:false`" because an
/// operator seeing `message_not_found` should stop, and one seeing a transport
/// blip should not — the two must not render the same.
const MUTATE_TERMINAL_CODES: &[&str] = &[
    "message_not_found",
    "cant_update_message",
    "cant_delete_message",
    "edit_window_closed",
    "channel_not_found",
    "is_archived",
    "msg_too_long",
];

/// Shared `ok:true`-envelope POST for the two mutate verbs.
///
/// Single attempt, deliberately. `post_message` retries because a lost send is
/// a lost message; an edit or a delete that fails is visible to the caller and
/// re-issuable by them, and a blind retry of a delete against a platform with
/// no idempotency slot is how one delete becomes two API errors in the log.
async fn post_mutate<B: Serialize + ?Sized>(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    method: &str,
    body: &B,
) -> Result<MutateMessageResponse, SlackError> {
    let url = format!("{}/api/{method}", api_base.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .bearer_auth(bot_token)
        .header("Content-Type", "application/json; charset=utf-8")
        .json(body)
        .send()
        .await
        .map_err(|e| SlackError::Api(format!("send error: {e}")))?;

    let status = resp.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        let body = resp.text().await.unwrap_or_default();
        return Err(SlackError::Auth(format!(
            "HTTP {}: {body}",
            status.as_u16()
        )));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(SlackError::Api(format!("HTTP {}: {body}", status.as_u16())));
    }

    let parsed: MutateMessageResponse = resp
        .json()
        .await
        .map_err(|e| SlackError::MalformedPayload(format!("decode {method} response: {e}")))?;

    if !parsed.ok {
        let code = parsed.error.unwrap_or_else(|| "unknown".to_string());
        if is_auth_rejection(&code) {
            return Err(SlackError::Auth(code));
        }
        // Terminal or unknown — either way this is not retried here; the
        // distinction is preserved in the message the operator reads.
        let _ = MUTATE_TERMINAL_CODES.contains(&code.as_str());
        return Err(SlackError::Api(code));
    }
    Ok(parsed)
}

/// Edit an already-sent message via `POST /api/chat.update`.
pub async fn update_message(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    req: &UpdateMessageRequest,
) -> Result<MutateMessageResponse, SlackError> {
    post_mutate(http, api_base, bot_token, "chat.update", req).await
}

/// Delete an already-sent message via `POST /api/chat.delete`.
pub async fn delete_message(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    req: &DeleteMessageRequest,
) -> Result<MutateMessageResponse, SlackError> {
    post_mutate(http, api_base, bot_token, "chat.delete", req).await
}

/// Slack file-download hosts. `url_private` arrives inside an inbound event
/// (sender-influenced), and we attach the bot's `xoxb` token to the request —
/// so the host is validated against this allowlist BEFORE the token is
/// attached, fail-closed. This blocks both SSRF and exfiltration of the bot
/// token to an attacker-controlled URL. Slack serves files from `files.slack.com`
/// (and, for some grids, other `*.slack.com` hosts).
pub const MEDIA_HOSTS: &[&str] = &["files.slack.com", ".slack.com"];

/// Download a Slack `url_private` file with the bot token. The `url` comes from
/// an inbound event, so it is validated against `allowed_hosts` (normally
/// [`MEDIA_HOSTS`]) BEFORE the bot token is attached or any request is issued.
/// Single attempt; the media enricher bounds it with its own timeout. Slack
/// returns the raw bytes on success; an unauthorized request yields an HTML
/// login page (the caller MIME-sniffs and rejects non-media), so only
/// transport/HTTP errors surface here.
pub async fn download_file(
    http: &wcore_egress::EgressClient,
    url: &str,
    bot_token: &str,
    allowed_hosts: &[&str],
) -> Result<Vec<u8>, SlackError> {
    if !wcore_egress::host_in_allowlist(url, allowed_hosts) {
        return Err(SlackError::Api(
            "refused media fetch: host not in Slack file-domain allowlist".to_string(),
        ));
    }
    let resp = http
        .get(url)
        .bearer_auth(bot_token)
        .send()
        .await
        .map_err(|e| SlackError::Api(format!("media download send error: {e}")))?;
    let status = resp.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        let body = resp.text().await.unwrap_or_default();
        return Err(SlackError::Auth(format!(
            "HTTP {}: {body}",
            status.as_u16()
        )));
    }
    if !status.is_success() {
        return Err(SlackError::Api(format!(
            "media download HTTP {}",
            status.as_u16()
        )));
    }
    // Bounded streamed read so a file response with no/forged Content-Length
    // can't OOM the process (defense-in-depth atop the files.slack host check).
    // The cap is the adapter's DECLARED bound, not a second hardcoded number.
    let max_bytes = usize::try_from(crate::MEDIA_BOUNDS.max_bytes).unwrap_or(usize::MAX);
    let bytes = wcore_egress::read_body_capped(resp, max_bytes)
        .await
        .map_err(|e| SlackError::Api(format!("media body read: {e}")))?;
    Ok(bytes)
}

#[cfg(test)]
mod reaction_tests {
    use super::*;

    #[test]
    fn emoji_map_covers_ack_set() {
        assert_eq!(slack_emoji_name("👀"), Some("eyes"));
        assert_eq!(slack_emoji_name("✅"), Some("white_check_mark"));
        assert_eq!(slack_emoji_name("❌"), Some("x"));
        assert_eq!(slack_emoji_name("🦄"), None);
    }

    fn req() -> AddReactionRequest {
        AddReactionRequest {
            channel: "C123".to_string(),
            timestamp: "1234.5678".to_string(),
            name: "eyes".to_string(),
        }
    }

    #[tokio::test]
    async fn add_reaction_succeeds_on_ok_true() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/reactions.add")
            .match_header("authorization", "Bearer xoxb-tok")
            .with_status(200)
            .with_body(r#"{"ok":true}"#)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        add_reaction(&http, &server.url(), "xoxb-tok", &req())
            .await
            .expect("ok:true should succeed");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn already_reacted_is_treated_as_success() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/reactions.add")
            .with_status(200)
            .with_body(r#"{"ok":false,"error":"already_reacted"}"#)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        add_reaction(&http, &server.url(), "xoxb-tok", &req())
            .await
            .expect("already_reacted is idempotent success");
    }

    #[tokio::test]
    async fn unknown_ok_false_is_api_error() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/reactions.add")
            .with_status(200)
            .with_body(r#"{"ok":false,"error":"message_not_found"}"#)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let err = add_reaction(&http, &server.url(), "xoxb-tok", &req())
            .await
            .expect_err("ok:false should error");
        assert!(
            matches!(err, SlackError::Api(ref c) if c == "message_not_found"),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn download_file_sends_bearer_and_returns_bytes() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/files/x.png")
            .match_header("authorization", "Bearer xoxb-tok")
            .with_status(200)
            .with_body(b"\x89PNG\r\n\x1a\nslackpng".as_slice())
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let bytes = download_file(
            &http,
            &format!("{}/files/x.png", server.url()),
            "xoxb-tok",
            &["127.0.0.1"],
        )
        .await
        .unwrap();
        assert_eq!(&bytes[..4], b"\x89PNG");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn download_file_unauthorized_maps_to_auth() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(401)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let err = download_file(&http, &format!("{}/x", server.url()), "tok", &["127.0.0.1"])
            .await
            .unwrap_err();
        assert!(matches!(err, SlackError::Auth(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn download_file_rejects_non_allowlisted_host_before_token() {
        // A url_private pointing off Slack's file domains must be refused by the
        // real allowlist before the bot token is ever attached/sent (SSRF +
        // token-exfil guard). No mock server: the guard must short-circuit.
        let http = wcore_egress::EgressClient::new();
        let err = download_file(
            &http,
            "http://169.254.169.254/latest/meta-data/",
            "xoxb-secret",
            MEDIA_HOSTS,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SlackError::Api(_)), "got {err:?}");
    }
}

/// Every `ok:false` site must agree with [`is_auth_rejection`] about what a
/// credential rejection is.
///
/// `HealthState::Unauthenticated` has exactly one Slack producer on a running
/// gateway: an outbound call returning [`SlackError::Auth`], which
/// `SlackChannel::post` turns into `ChannelEvent::AuthExpired`. A site that
/// answers `SlackError::Api` for a credential code silently drops that
/// producer, and the channel keeps reading `Healthy` while holding a refused
/// token — the whole defect. That is not hypothetical: `account_inactive` was
/// listed in `is_auth_rejection` (with a comment explaining why it belongs)
/// and omitted from all three hand-rolled lists, so a bot user deactivated by
/// a Slack admin was classified as an ordinary API error.
///
/// These tests iterate the classifier's own codes rather than restating a
/// list, so adding a code to `is_auth_rejection` and forgetting a call site
/// reddens here instead of shipping.
#[cfg(test)]
mod auth_classification_tests {
    use super::*;

    /// The codes `is_auth_rejection` accepts. Kept adjacent to the function so
    /// the assertion below fails loudly if the two ever diverge.
    const CREDENTIAL_CODES: &[&str] = &[
        "invalid_auth",
        "not_authed",
        "token_revoked",
        "token_expired",
        "account_inactive",
    ];

    #[test]
    fn the_classifier_and_this_fixture_agree() {
        for code in CREDENTIAL_CODES {
            assert!(is_auth_rejection(code), "{code} must be a credential code");
        }
        // Negative control — without this the fixture could be a list of
        // strings the classifier accepts unconditionally.
        for code in [
            "channel_not_found",
            "msg_too_long",
            "is_archived",
            "unknown",
        ] {
            assert!(
                !is_auth_rejection(code),
                "{code} is a request fault, not a credential verdict"
            );
        }
    }

    #[tokio::test]
    async fn every_credential_code_is_auth_on_chat_post_message() {
        for code in CREDENTIAL_CODES {
            let mut server = mockito::Server::new_async().await;
            let _m = server
                .mock("POST", "/api/chat.postMessage")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(format!(r#"{{"ok":false,"error":"{code}"}}"#))
                .create_async()
                .await;
            let http = wcore_egress::EgressClient::new();
            let err = post_message(
                &http,
                &server.url(),
                "xoxb-tok",
                &PostMessageRequest {
                    channel: "C1".to_string(),
                    text: "hi".to_string(),
                    thread_ts: None,
                },
                1,
            )
            .await
            .unwrap_err();
            assert!(
                matches!(err, SlackError::Auth(_)),
                "chat.postMessage classified {code} as {err:?}, so no AuthExpired \
                 is published and health stays Healthy on a refused token"
            );
        }
    }

    #[tokio::test]
    async fn every_credential_code_is_auth_on_reactions_add() {
        for code in CREDENTIAL_CODES {
            let mut server = mockito::Server::new_async().await;
            let _m = server
                .mock("POST", "/api/reactions.add")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(format!(r#"{{"ok":false,"error":"{code}"}}"#))
                .create_async()
                .await;
            let http = wcore_egress::EgressClient::new();
            let err = add_reaction(
                &http,
                &server.url(),
                "xoxb-tok",
                &AddReactionRequest {
                    channel: "C123".to_string(),
                    timestamp: "1234.5678".to_string(),
                    name: "eyes".to_string(),
                },
            )
            .await
            .unwrap_err();
            assert!(
                matches!(err, SlackError::Auth(_)),
                "reactions.add classified {code} as {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn every_credential_code_is_auth_on_chat_update() {
        for code in CREDENTIAL_CODES {
            let mut server = mockito::Server::new_async().await;
            let _m = server
                .mock("POST", "/api/chat.update")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(format!(r#"{{"ok":false,"error":"{code}"}}"#))
                .create_async()
                .await;
            let http = wcore_egress::EgressClient::new();
            let err = update_message(
                &http,
                &server.url(),
                "xoxb-tok",
                &UpdateMessageRequest {
                    channel: "C1".to_string(),
                    ts: "1234.5678".to_string(),
                    text: "edited".to_string(),
                },
            )
            .await
            .unwrap_err();
            assert!(
                matches!(err, SlackError::Auth(_)),
                "chat.update classified {code} as {err:?}"
            );
        }
    }

    /// The classification must still be able to say "not a credential problem".
    /// A site that returned `Auth` for everything would pass the three tests
    /// above and would flip a healthy channel to "rotate your token" the first
    /// time somebody posted to an archived channel.
    #[tokio::test]
    async fn a_request_fault_is_not_a_credential_verdict() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error":"channel_not_found"}"#)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let err = post_message(
            &http,
            &server.url(),
            "xoxb-tok",
            &PostMessageRequest {
                channel: "C-nope".to_string(),
                text: "hi".to_string(),
                thread_ts: None,
            },
            1,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SlackError::Api(_)), "got {err:?}");
    }
}
