//! Thin reqwest wrapper around the Discord REST API surface this
//! adapter needs: `POST /channels/{channel_id}/messages` for outbound.
//!
//! The helper takes an explicit `api_base` so tests can point at a
//! `mockito::Server` instead of `https://discord.com`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::DiscordError;

/// Number of retry attempts (including the first one) for outbound sends.
pub(crate) const SEND_MAX_ATTEMPTS: u32 = 5;
/// Base backoff for transient retries.
pub(crate) const SEND_BASE_BACKOFF_MS: u64 = 200;
/// Cap any single sleep between retries — both transient backoff and
/// 429 retry_after collapse to this on the high side so a malicious or
/// buggy server can't park us indefinitely.
pub(crate) const SEND_MAX_BACKOFF_MS: u64 = 30_000;

/// Subset of the Discord v10 Message object the adapter consumes.
/// Unknown fields are tolerated so future API additions don't break us.
#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    /// Snowflake string.
    pub id: String,
    /// Discord uses ISO-8601 strings for timestamps. We carry the raw
    /// string and convert to epoch seconds at the call site so we don't
    /// pull `chrono` into this hot path.
    #[serde(default)]
    pub timestamp: Option<String>,
    /// Snowflake string for the channel the message was posted in.
    #[serde(default)]
    pub channel_id: Option<String>,
}

/// `POST /channels/{channel_id}/messages` request body.
#[derive(Debug, Clone, Serialize)]
pub struct CreateMessageBody<'a> {
    pub content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_reference: Option<MessageReference<'a>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageReference<'a> {
    pub message_id: &'a str,
}

/// Shape of a Discord error response (4xx). All fields optional — older
/// errors only carry `message`; newer ones carry `code` and `errors`.
/// `code` is captured for diagnostics even though the adapter doesn't
/// branch on it (the HTTP status carries the routing signal).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ErrorResponse {
    #[serde(default)]
    #[allow(dead_code)]
    pub code: Option<u64>,
    #[serde(default)]
    pub message: Option<String>,
}

/// Send one message through `POST /channels/{channel_id}/messages`.
///
/// Retry policy: up to `SEND_MAX_ATTEMPTS` total tries, exponential
/// backoff on 5xx / network failure, `Retry-After` honoured on 429
/// (Discord sends seconds, sometimes as a float, in both the header and
/// the JSON body — header wins), permanent-error short-circuit on any
/// other 4xx.
pub(crate) async fn send_message(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    channel_id: &str,
    body: &CreateMessageBody<'_>,
) -> Result<Message, DiscordError> {
    let url = format!("{api_base}/api/v10/channels/{channel_id}/messages");
    let auth = format!("Bot {bot_token}");
    let mut last_err = DiscordError::Http("no attempts made".to_string());
    let mut last_retry_after: f64 = 0.0;

    for attempt in 0..SEND_MAX_ATTEMPTS {
        if attempt > 0 {
            let sleep_ms = exp_backoff_ms(attempt);
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
        }

        let resp = match http
            .post(&url)
            .header(reqwest::header::AUTHORIZATION, &auth)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_err = DiscordError::Http(format!("network: {e}"));
                continue;
            }
        };

        let status = resp.status();

        // 429 — honour Retry-After header (Discord sends seconds; may be float).
        if status.as_u16() == 429 {
            let retry_after_secs = extract_retry_after(&resp);
            last_retry_after = retry_after_secs;
            let _ = resp.bytes().await; // drain body
            let sleep_ms = ((retry_after_secs.max(0.0) * 1000.0) as u64).min(SEND_MAX_BACKOFF_MS);
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
            last_err = DiscordError::RateLimited { retry_after_secs };
            continue;
        }

        // 4xx (except 429) is permanent — short-circuit.
        if status.is_client_error() {
            let bytes = resp.bytes().await.unwrap_or_default();
            let err: ErrorResponse = serde_json::from_slice(&bytes).unwrap_or_default();
            let desc = err.message.unwrap_or_else(|| format!("status {status}"));
            // 401 / 403 are auth.
            if matches!(status.as_u16(), 401 | 403) {
                return Err(DiscordError::Auth(desc));
            }
            return Err(DiscordError::Rejected {
                code: status.as_u16(),
                description: desc,
            });
        }

        // 5xx — transient, back off and retry.
        if status.is_server_error() {
            let _ = resp.bytes().await;
            last_err = DiscordError::Http(format!("server {status}"));
            continue;
        }

        // 2xx — parse the message.
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => {
                last_err = DiscordError::Http(format!("body read: {e}"));
                continue;
            }
        };
        let msg: Message = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => return Err(DiscordError::Decode(e.to_string())),
        };
        return Ok(msg);
    }

    // Exhausted attempts. If the last failure was a 429, surface that.
    if last_retry_after > 0.0 {
        return Err(DiscordError::RateLimited {
            retry_after_secs: last_retry_after,
        });
    }
    Err(last_err)
}

/// Pull the `Retry-After` value (seconds, possibly fractional) out of a
/// response. Falls back to 1.0 if the header is missing / unparseable —
/// the caller's retry budget bounds how long we end up sleeping.
fn extract_retry_after(resp: &reqwest::Response) -> f64 {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|f| f.is_finite() && *f >= 0.0)
        .unwrap_or(1.0)
}

fn exp_backoff_ms(attempt: u32) -> u64 {
    // attempt=1 -> 200ms, attempt=2 -> 400ms, attempt=3 -> 800ms, ...
    let shift = attempt.saturating_sub(1).min(10);
    SEND_BASE_BACKOFF_MS
        .saturating_mul(1u64 << shift)
        .min(SEND_MAX_BACKOFF_MS)
}

/// Parse a Discord ISO-8601 timestamp string into epoch seconds.
/// Returns 0 if the string is missing or unparseable — callers use the
/// receipt's `ts_secs` only for ordering / display, never for security.
pub(crate) fn parse_iso8601_to_epoch(ts: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

/// Trigger the typing indicator in a channel via
/// `POST /channels/{channel_id}/typing` (empty body, `204 No Content`).
///
/// Best-effort and single-shot: the subscriber calls this periodically
/// while a turn runs, so retrying a missed indicator is pointless — the
/// next keepalive tick supersedes it. A failure is mapped to a structured
/// error the caller logs and ignores.
pub(crate) async fn trigger_typing(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    channel_id: &str,
) -> Result<(), DiscordError> {
    let url = format!("{api_base}/api/v10/channels/{channel_id}/typing");
    let auth = format!("Bot {bot_token}");
    let resp = http
        .post(&url)
        .header(reqwest::header::AUTHORIZATION, &auth)
        .send()
        .await
        .map_err(|e| DiscordError::Http(format!("network: {e}")))?;
    status_to_result(resp.status(), "typing")
}

/// Add a single-emoji reaction to a message via
/// `PUT /channels/{channel_id}/messages/{message_id}/reactions/{emoji}/@me`
/// (`204 No Content`). The unicode emoji is percent-encoded into the path.
pub(crate) async fn add_reaction(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    channel_id: &str,
    message_id: &str,
    emoji: &str,
) -> Result<(), DiscordError> {
    let encoded = percent_encode(emoji);
    let url = format!(
        "{api_base}/api/v10/channels/{channel_id}/messages/{message_id}/reactions/{encoded}/@me"
    );
    let auth = format!("Bot {bot_token}");
    let resp = http
        .put(&url)
        .header(reqwest::header::AUTHORIZATION, &auth)
        .send()
        .await
        .map_err(|e| DiscordError::Http(format!("network: {e}")))?;
    status_to_result(resp.status(), "reaction")
}

/// Classify a bodyless Discord response: 2xx → Ok, 401/403 → Auth,
/// anything else → Rejected. Used by the best-effort typing/reaction calls
/// that don't parse a response body.
fn status_to_result(status: reqwest::StatusCode, op: &str) -> Result<(), DiscordError> {
    if status.is_success() {
        Ok(())
    } else if matches!(status.as_u16(), 401 | 403) {
        Err(DiscordError::Auth(format!("{op}: status {status}")))
    } else {
        Err(DiscordError::Rejected {
            code: status.as_u16(),
            description: format!("{op}: status {status}"),
        })
    }
}

/// Percent-encode a string's UTF-8 bytes, leaving only the RFC 3986
/// unreserved set untouched — enough to put a unicode emoji safely into a
/// URL path segment. Kept local to avoid pulling in a urlencoding dep.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => {
                out.push('%');
                out.push(
                    char::from_digit((byte >> 4) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit((byte & 0xf) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod reaction_tests {
    use super::*;

    #[test]
    fn percent_encode_emoji_encodes_utf8_bytes() {
        // 👀 is U+1F440 → UTF-8 F0 9F 91 80.
        assert_eq!(percent_encode("👀"), "%F0%9F%91%80");
        // ✅ is U+2705 → UTF-8 E2 9C 85.
        assert_eq!(percent_encode("✅"), "%E2%9C%85");
        // ASCII unreserved passes through.
        assert_eq!(percent_encode("aZ9-_.~"), "aZ9-_.~");
    }

    #[tokio::test]
    async fn trigger_typing_succeeds_on_204() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/v10/channels/chan1/typing")
            .match_header("authorization", "Bot tok")
            .with_status(204)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        trigger_typing(&http, &server.url(), "tok", "chan1")
            .await
            .expect("typing should succeed on 204");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn add_reaction_puts_encoded_emoji_and_succeeds_on_204() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "PUT",
                "/api/v10/channels/chan1/messages/msg1/reactions/%F0%9F%91%80/@me",
            )
            .match_header("authorization", "Bot tok")
            .with_status(204)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        add_reaction(&http, &server.url(), "tok", "chan1", "msg1", "👀")
            .await
            .expect("reaction should succeed on 204");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn reaction_forbidden_maps_to_auth() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("PUT", mockito::Matcher::Any)
            .with_status(403)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let err = add_reaction(&http, &server.url(), "tok", "c", "m", "👀")
            .await
            .expect_err("403 should error");
        assert!(matches!(err, DiscordError::Auth(_)), "got {err:?}");
    }
}
