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
    /// Optimistic-send dedup token (≤25 chars). Discord deduplicates
    /// create-message requests that carry the same `nonce` from the same
    /// author within a short window, so reusing ONE nonce across the retry
    /// loop turns a retry-after-a-lost-success into a no-op instead of a
    /// duplicate post (HIGH-7).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<&'a str>,
}

/// Discord's `nonce` is capped at 25 characters.
const MAX_NONCE_LEN: usize = 25;

/// Generate a process-unique nonce for a send with no delivery key.
///
/// The global monotonic counter guarantees uniqueness within a process and the
/// wall-clock millis prefix keeps it distinct across restarts. **That
/// distinctness is correct HERE and wrong for a keyed send** — an unkeyed
/// message has no logical identity to collapse against, so presenting a stable
/// nonce would let Discord suppress a genuinely new message. A keyed send uses
/// [`nonce_for_key`] instead.
///
/// The `-` separator makes the `{ms}-{n}` concatenation unambiguous, and the
/// whole string stays well under the cap.
pub(crate) fn next_nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    format!("{ms:x}-{n:x}")
}

/// Derive a nonce from the gateway's outbound delivery key.
///
/// The nonce field exists so Discord can collapse a re-sent create-message
/// request. Until this existed the adapter only ever produced [`next_nonce`],
/// whose documented property was that it is *"distinct across restarts"* — so
/// the token deliberately differed on exactly the replay it was there to
/// suppress, and the field bought nothing beyond a single process's retry loop.
///
/// The delivery key is hashed rather than truncated: Discord caps the nonce at
/// 25 characters, and a delivery key is `cron:{job_id}:{epoch_millis}` whose
/// DISTINGUISHING tail (the timestamp) is what truncation would cut off — two
/// occurrences of one job would collapse into a single nonce and the second
/// would be suppressed as a duplicate. That is a message loss, so truncation is
/// not an option here.
pub(crate) fn nonce_for_key(key: &str) -> String {
    // FNV-1a, 64-bit. Collision avoidance, not a security boundary.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let n = format!("wl{h:016x}");
    debug_assert!(n.len() <= MAX_NONCE_LEN);
    n
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
/// Resolve this bot's own stable user id via `GET /users/@me`. Called at
/// `start()` so the gateway can do precise `is_self` / mention detection —
/// without it every guild message is conservatively non-self and non-mention,
/// so a mention-gated server bot can never admit a turn.
pub(crate) async fn get_current_user_id(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
) -> Result<String, DiscordError> {
    let url = format!("{api_base}/api/v10/users/@me");
    let auth = format!("Bot {bot_token}");
    let resp = http
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, &auth)
        .send()
        .await
        .map_err(|e| DiscordError::Http(format!("network: {e}")))?;
    if !resp.status().is_success() {
        return Err(DiscordError::Auth(format!(
            "GET /users/@me returned HTTP {}",
            resp.status().as_u16()
        )));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| DiscordError::Decode(format!("/users/@me decode: {e}")))?;
    body.get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| DiscordError::Decode("/users/@me: missing id field".to_string()))
}

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

/// Discord media CDN hosts. Inbound `attachment.url` values are attacker-
/// controlled (they arrive verbatim in gateway JSON), so media fetches are
/// confined to these hosts — fail-closed against SSRF to internal/metadata
/// targets. Discord serves all attachment media from exactly these two.
pub(crate) const MEDIA_HOSTS: &[&str] = &["cdn.discordapp.com", "media.discordapp.net"];

/// Download a public Discord CDN media URL (no Authorization — the CDN is
/// public; sending the bot token would be wrong). The `url` originates from an
/// inbound message, so it is validated against `allowed_hosts` (normally
/// [`MEDIA_HOSTS`]) before any request is issued. Single attempt; the media
/// enricher bounds it with its own timeout.
pub(crate) async fn download_bytes(
    http: &wcore_egress::EgressClient,
    url: &str,
    allowed_hosts: &[&str],
) -> Result<Vec<u8>, DiscordError> {
    if !wcore_egress::host_in_allowlist(url, allowed_hosts) {
        return Err(DiscordError::Rejected {
            code: 0,
            description: "refused media fetch: host not in Discord CDN allowlist".to_string(),
        });
    }
    let resp = http
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| DiscordError::Http(format!("network: {e}")))?;
    if !resp.status().is_success() {
        return Err(DiscordError::Http(format!(
            "media download returned HTTP {}",
            resp.status().as_u16()
        )));
    }
    // Stream the body with a hard cap so a CDN response that omits/lies about
    // Content-Length can't buffer unbounded into an OOM (defense-in-depth on
    // top of the CDN host allowlist above).
    //
    // The cap is the adapter's DECLARED bound, not a second hardcoded number.
    // It used to be a private `const MAX_MEDIA_BYTES = 100 MiB` while
    // `media_bounds()` advertised 25 MiB to anyone who asked — an advertised
    // limit no code path read.
    let max_bytes = usize::try_from(crate::MEDIA_BOUNDS.max_bytes).unwrap_or(usize::MAX);
    let bytes = wcore_egress::read_body_capped(resp, max_bytes)
        .await
        .map_err(|e| DiscordError::Http(format!("media body read: {e}")))?;
    Ok(bytes)
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

    #[test]
    fn next_nonce_is_unique_and_within_cap() {
        let a = next_nonce();
        let b = next_nonce();
        assert_ne!(a, b, "nonces must be unique per call");
        assert!(
            a.len() <= 25 && b.len() <= 25,
            "nonce must fit Discord's 25-char cap"
        );
    }

    #[tokio::test]
    async fn send_reuses_one_nonce_across_a_retry() {
        // First attempt 503 (transient → retry), second 200. Both requests
        // must carry the SAME nonce so Discord dedupes the retry.
        let mut server = mockito::Server::new_async().await;
        let nonce = next_nonce();
        let body_matcher =
            mockito::Matcher::Regex(format!(r#""nonce":"{}""#, regex_escape(&nonce)));
        let first = server
            .mock("POST", "/api/v10/channels/c1/messages")
            .match_body(body_matcher.clone())
            .with_status(503)
            .expect(1)
            .create_async()
            .await;
        let second = server
            .mock("POST", "/api/v10/channels/c1/messages")
            .match_body(body_matcher)
            .with_status(200)
            .with_body(r#"{"id":"m1","channel_id":"c1"}"#)
            .expect(1)
            .create_async()
            .await;

        let http = wcore_egress::EgressClient::new();
        let body = CreateMessageBody {
            content: "hi",
            message_reference: None,
            nonce: Some(&nonce),
        };
        let msg = send_message(&http, &server.url(), "tok", "c1", &body)
            .await
            .expect("retry should succeed on the 200");
        assert_eq!(msg.id, "m1");
        // Both mocks asserting proves the identical nonce went out twice.
        first.assert_async().await;
        second.assert_async().await;
    }

    /// Escape the regex metacharacter that can appear in a nonce (`-` is
    /// literal in a char class but safe outside; the only other is none —
    /// the nonce is `[0-9a-f-]`). Kept tiny and local.
    fn regex_escape(s: &str) -> String {
        s.replace('-', r"\-")
    }

    /// The property that makes the nonce worth anything: the SAME logical
    /// delivery produces the SAME nonce from a DIFFERENT process.
    ///
    /// `next_nonce` cannot do this — its documented behaviour is to be
    /// "distinct across restarts", which defeats the only replay Discord's
    /// nonce field could suppress. This asserts the opposite property for the
    /// keyed derivation, and asserts that `next_nonce` still does NOT have it,
    /// because an unkeyed send must not present a collapsible token.
    #[test]
    fn the_keyed_nonce_is_stable_across_processes_and_the_unkeyed_one_is_not() {
        let a = nonce_for_key("cron:job-a:1785121776528");
        let again = nonce_for_key("cron:job-a:1785121776528");
        assert_eq!(a, again, "the same delivery must map to the same nonce");

        // A different occurrence of the SAME job must NOT collapse. Truncating
        // the key instead of hashing it would break exactly this, because the
        // distinguishing timestamp is the TAIL.
        assert_ne!(a, nonce_for_key("cron:job-a:1785121776529"));
        assert_ne!(a, nonce_for_key("cron:job-b:1785121776528"));

        assert!(
            a.len() <= MAX_NONCE_LEN,
            "nonce must fit Discord's {MAX_NONCE_LEN}-char cap, got {} for {a}",
            a.len()
        );

        // And the unkeyed generator keeps its distinctness, which is correct
        // for a send with no logical identity.
        assert_ne!(next_nonce(), next_nonce());
    }

    #[tokio::test]
    async fn download_bytes_fetches_cdn_media() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/attachments/1/2/x.png")
            .with_status(200)
            .with_body(b"\x89PNG\r\n\x1a\npngbytes".as_slice())
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let bytes = download_bytes(
            &http,
            &format!("{}/attachments/1/2/x.png", server.url()),
            &["127.0.0.1"],
        )
        .await
        .unwrap();
        assert_eq!(&bytes[..4], b"\x89PNG");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn download_bytes_errors_on_404() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(404)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        assert!(
            download_bytes(&http, &format!("{}/x", server.url()), &["127.0.0.1"])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn download_bytes_rejects_non_cdn_host_without_request() {
        // An inbound attachment URL pointing at cloud metadata (SSRF) must be
        // refused by the real CDN allowlist before any network call is made.
        let http = wcore_egress::EgressClient::new();
        let err = download_bytes(
            &http,
            "http://169.254.169.254/latest/meta-data/iam/",
            MEDIA_HOSTS,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DiscordError::Rejected { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn get_current_user_id_parses_id_from_users_me() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/v10/users/@me")
            .match_header("authorization", "Bot tok")
            .with_status(200)
            .with_body(r#"{"id":"998877","username":"acme-bot"}"#)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let id = get_current_user_id(&http, &server.url(), "tok")
            .await
            .unwrap();
        assert_eq!(id, "998877");
        mock.assert_async().await;
    }
}
