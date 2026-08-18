//! Matrix CS API REST helpers.
//!
//! Implements the send path: `PUT /_matrix/client/v3/rooms/{roomId}/send/m.room.message/{txnId}`.
//!
//! # The transaction id, and why a counter was the wrong source
//!
//! Matrix deduplicates a `PUT ... /send/{eventType}/{txnId}` by
//! `(access token, txnId)`: re-sending the same pair returns the ORIGINAL
//! `event_id` and posts nothing. That is a genuine idempotency primitive —
//! most platforms have none — and this adapter was throwing it away.
//!
//! The transaction id came from a process-local `AtomicU64` seeded at 1, so it
//! RESET on every restart. That breaks the primitive in both directions:
//!
//! - **No dedup where it matters.** The replay a restart has to worry about is
//!   the delivery whose outcome is unknown because the process died
//!   mid-attempt. A counter that restarts cannot recognise that delivery,
//!   which is precisely the case it existed to cover.
//! - **False dedup, i.e. LOSS.** Worse, and measured — see
//!   `24-C1-ABANDON-SURFACE.md`. After a restart the counter re-issues
//!   `1, 2, 3...` against the same access token. A homeserver that still holds
//!   those transaction ids treats a genuinely NEW message as a replay and
//!   silently drops it, returning the OLD event's id. Nothing errors; the
//!   message simply never appears.
//!
//! So the id is now derived from the caller's delivery key, which is stable
//! across restarts for one logical delivery and distinct across different ones
//! — the two properties the field actually requires. Unkeyed sends keep a
//! process-local unique id, because an ordinary send has no logical identity to
//! collapse against and must never present a stable one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::MatrixError;

/// Fallback source of transaction ids for sends with no delivery key.
///
/// Seeded from the wall clock rather than from 1 so an unkeyed send after a
/// restart cannot collide with a transaction id the homeserver still holds and
/// be silently dropped as a replay. This is the loss path above; unkeyed sends
/// are subject to it too, and a fresh process must not re-walk ids it already
/// used.
static TXN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Longest transaction id this adapter will put in a URL path segment.
const MAX_TXN_LEN: usize = 64;

/// Derive a Matrix transaction id from an outbound delivery key.
///
/// Requirements, in order: **stable** across restarts for the same logical
/// delivery (or the homeserver cannot recognise the replay), **distinct**
/// across different deliveries (or the homeserver drops a new message as a
/// replay), and safe in a URL path segment.
///
/// The key is used directly when it is already short and path-safe, so the
/// wire stays legible during an incident — `cron:job-a:1785121776528` is
/// something an operator can match against the ledger by eye. Anything longer
/// or containing a character that would need escaping is hashed instead, which
/// preserves both properties without the escaping question.
pub(crate) fn txn_id_for_key(key: &str) -> String {
    let path_safe = !key.is_empty()
        && key.len() <= MAX_TXN_LEN
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'));
    if path_safe {
        return key.to_string();
    }
    // FNV-1a, 64-bit. A non-cryptographic hash is right here: this is a
    // collision-avoidance identifier, not a security boundary, and two
    // different deliveries colliding would merely re-raise the duplicate
    // question the ledger already arbitrates.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("wl-{h:016x}")
}

/// A transaction id for a send that carries no delivery key.
fn next_unkeyed_txn_id() -> String {
    let n = TXN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // Millis FIRST so a restart cannot re-walk ids a previous process used.
    format!("wl-u{ms:x}-{n:x}")
}

/// Hard cap on a single media download. The `mxc://` URI is attacker-controlled
/// (it arrives on an inbound message), so the body is streamed with a byte cap
/// to prevent an OOM-DoS from a homeserver that omits/lies about
/// `Content-Length`. Matches the 100 MiB cap used by the Discord/Slack/Telegram
/// media paths.
///
/// Derived from the adapter's DECLARED bound rather than being a second
/// hardcoded number, so `media_bounds()` cannot advertise one figure while this
/// path enforces another.
const MAX_MEDIA_BYTES: usize = crate::MEDIA_BOUNDS.max_bytes as usize;

/// Wall-clock timeout for a media download request (including the body read).
const MEDIA_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Serialize)]
struct TextMessageBody<'a> {
    msgtype: &'a str,
    body: &'a str,
}

#[derive(Deserialize)]
struct SendEventResponse {
    event_id: String,
}

/// Send a plain-text `m.room.message` to `room_id` and return the server-assigned `event_id`.
///
/// `delivery_key` is the gateway's outbound idempotency key. `Some` makes the
/// transaction id stable across restarts for that logical delivery, which is
/// what lets the homeserver collapse a replay; `None` gets a process-unique id.
pub async fn send_text_message(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    access_token: &str,
    room_id: &str,
    body: &str,
    delivery_key: Option<&str>,
) -> Result<String, MatrixError> {
    let txn_id = match delivery_key {
        Some(k) => txn_id_for_key(k),
        None => next_unkeyed_txn_id(),
    };
    let encoded_room = urlencoding::encode(room_id);
    let url =
        format!("{api_base}/_matrix/client/v3/rooms/{encoded_room}/send/m.room.message/{txn_id}");

    let payload = TextMessageBody {
        msgtype: "m.text",
        body,
    };

    let resp = http
        .put(&url)
        .bearer_auth(access_token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| MatrixError::Network(e.to_string()))?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(MatrixError::Http { status, body: text });
    }

    let result: SendEventResponse = resp
        .json()
        .await
        .map_err(|e| MatrixError::Parse(e.to_string()))?;

    Ok(result.event_id)
}

/// The token pair `POST /_matrix/client/v3/refresh` returns.
#[derive(Debug, Deserialize)]
pub(crate) struct RefreshedTokens {
    pub access_token: String,
    /// Matrix refresh tokens ROTATE. When this is present the token we
    /// presented is SPENT, and persisting the replacement is not optional:
    /// a later process that replays the spent one can have the whole grant
    /// revoked (RFC 6819 §5.2.2.3).
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Lifetime of the new access token. Absent means the homeserver did not
    /// state one, so no proactive renewal is scheduled — the reactive 401 path
    /// still covers it.
    #[serde(default)]
    pub expires_in_ms: Option<u64>,
}

#[derive(Serialize)]
struct RefreshRequest<'a> {
    refresh_token: &'a str,
}

/// Exchange a refresh token for a fresh access token.
///
/// **Deliberately unauthenticated.** The refresh token IS the credential here
/// and rides in the body; attaching the (expired) access token as a bearer
/// would hand the homeserver a reason to 401 a call that must succeed, and
/// would make the dead credential a precondition for replacing itself.
pub(crate) async fn refresh_access_token(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    refresh_token: &str,
) -> Result<RefreshedTokens, MatrixError> {
    let url = format!("{api_base}/_matrix/client/v3/refresh");
    let resp = http
        .post(&url)
        .json(&RefreshRequest { refresh_token })
        .timeout(crate::token::REFRESH_POST_TIMEOUT)
        .send()
        .await
        .map_err(|e| MatrixError::Network(e.to_string()))?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(MatrixError::Http { status, body });
    }

    resp.json()
        .await
        .map_err(|e| MatrixError::Parse(e.to_string()))
}

#[derive(Serialize)]
struct TypingBody {
    typing: bool,
    timeout: u64,
}

/// Send a typing notification: `PUT /_matrix/client/v3/rooms/{room}/typing/{userId}`.
/// `timeout_ms` is how long the server should show the indicator before
/// auto-clearing it; the subscriber re-sends on a shorter cadence.
pub async fn send_typing(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    access_token: &str,
    room_id: &str,
    user_id: &str,
    timeout_ms: u64,
) -> Result<(), MatrixError> {
    let encoded_room = urlencoding::encode(room_id);
    let encoded_user = urlencoding::encode(user_id);
    let url = format!("{api_base}/_matrix/client/v3/rooms/{encoded_room}/typing/{encoded_user}");
    let payload = TypingBody {
        typing: true,
        timeout: timeout_ms,
    };
    let resp = http
        .put(&url)
        .bearer_auth(access_token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| MatrixError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if resp.status().is_success() {
        Ok(())
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(MatrixError::Http { status, body })
    }
}

#[derive(Serialize)]
struct ReactionBody<'a> {
    #[serde(rename = "m.relates_to")]
    relates_to: RelatesTo<'a>,
}

#[derive(Serialize)]
struct RelatesTo<'a> {
    rel_type: &'a str,
    event_id: &'a str,
    key: &'a str,
}

/// Send an `m.reaction` annotation relating to `event_id` with `emoji` as
/// the key: `PUT /_matrix/client/v3/rooms/{room}/send/m.reaction/{txnId}`.
pub async fn send_reaction(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    access_token: &str,
    room_id: &str,
    event_id: &str,
    emoji: &str,
) -> Result<(), MatrixError> {
    let txn_id = TXN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let encoded_room = urlencoding::encode(room_id);
    let url = format!("{api_base}/_matrix/client/v3/rooms/{encoded_room}/send/m.reaction/{txn_id}");
    let payload = ReactionBody {
        relates_to: RelatesTo {
            rel_type: "m.annotation",
            event_id,
            key: emoji,
        },
    };
    let resp = http
        .put(&url)
        .bearer_auth(access_token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| MatrixError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if resp.status().is_success() {
        Ok(())
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(MatrixError::Http { status, body })
    }
}

/// The body of an `m.replace` edit event.
///
/// Matrix has no "update this event" verb. An edit is a **new event** that
/// declares itself a replacement of an older one, and clients render the
/// replacement in place. That shape has two consequences this adapter must
/// honour and neither is optional:
///
/// - The **fallback** `body` is conventionally prefixed `* ` so a client too old
///   to understand `m.replace` shows something intelligible rather than a
///   duplicate. Omitting it makes an edit look like a second message on old
///   clients — a silent duplicate, which is the failure mode this phase spends
///   most of its effort on.
/// - The authoritative new text lives in `m.new_content`. A client that
///   understands the relation reads that and ignores the fallback.
#[derive(Serialize)]
struct EditMessageBody<'a> {
    msgtype: &'a str,
    /// Fallback rendering for clients that do not understand `m.replace`.
    body: String,
    #[serde(rename = "m.new_content")]
    new_content: TextMessageBody<'a>,
    #[serde(rename = "m.relates_to")]
    relates_to: ReplaceRelation<'a>,
}

#[derive(Serialize)]
struct ReplaceRelation<'a> {
    rel_type: &'a str,
    event_id: &'a str,
}

/// Edit `event_id` in `room_id` by sending an `m.replace` relation.
///
/// Returns the **new** event's id — the id of the replacement event, not of the
/// original. Matrix genuinely has two ids here and collapsing them would be a
/// lie: the original still exists and is still addressable, and a caller that
/// wants to edit again must relate to the ORIGINAL, not to the replacement.
/// The caller therefore keeps the original id; this receipt records what was
/// created.
pub async fn edit_message(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    access_token: &str,
    room_id: &str,
    event_id: &str,
    new_text: &str,
) -> Result<String, MatrixError> {
    let txn_id = next_unkeyed_txn_id();
    let encoded_room = urlencoding::encode(room_id);
    let url =
        format!("{api_base}/_matrix/client/v3/rooms/{encoded_room}/send/m.room.message/{txn_id}");

    let payload = EditMessageBody {
        msgtype: "m.text",
        body: format!("* {new_text}"),
        new_content: TextMessageBody {
            msgtype: "m.text",
            body: new_text,
        },
        relates_to: ReplaceRelation {
            rel_type: "m.replace",
            event_id,
        },
    };

    let resp = http
        .put(&url)
        .bearer_auth(access_token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| MatrixError::Network(e.to_string()))?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(MatrixError::Http { status, body: text });
    }
    let result: SendEventResponse = resp
        .json()
        .await
        .map_err(|e| MatrixError::Parse(e.to_string()))?;
    Ok(result.event_id)
}

/// Redact `event_id` in `room_id`:
/// `PUT /_matrix/client/v3/rooms/{room}/redact/{eventId}/{txnId}`.
///
/// Redaction is Matrix's delete. It strips the event's content server-side and
/// federates the removal; the event stub remains in the timeline by design.
/// This adapter does not pretend otherwise — the operation reports success when
/// the homeserver accepted the redaction, which is the strongest guarantee the
/// protocol offers.
pub async fn redact_event(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    access_token: &str,
    room_id: &str,
    event_id: &str,
) -> Result<String, MatrixError> {
    let txn_id = next_unkeyed_txn_id();
    let encoded_room = urlencoding::encode(room_id);
    let encoded_event = urlencoding::encode(event_id);
    let url = format!(
        "{api_base}/_matrix/client/v3/rooms/{encoded_room}/redact/{encoded_event}/{txn_id}"
    );

    let resp = http
        .put(&url)
        .bearer_auth(access_token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| MatrixError::Network(e.to_string()))?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(MatrixError::Http { status, body: text });
    }
    let result: SendEventResponse = resp
        .json()
        .await
        .map_err(|e| MatrixError::Parse(e.to_string()))?;
    Ok(result.event_id)
}

/// Split an `mxc://server/mediaId` URI into `(server, mediaId)`.
fn parse_mxc(mxc: &str) -> Result<(&str, &str), MatrixError> {
    let rest = mxc
        .strip_prefix("mxc://")
        .ok_or_else(|| MatrixError::Parse(format!("not an mxc URI: {mxc}")))?;
    rest.split_once('/')
        .filter(|(s, m)| !s.is_empty() && !m.is_empty())
        .ok_or_else(|| MatrixError::Parse(format!("malformed mxc URI: {mxc}")))
}

/// Download unencrypted Matrix media by its `mxc://server/id` URI via the
/// authenticated media endpoint (Matrix v1.11+ / MSC3916):
/// `GET /_matrix/client/v1/media/download/{server}/{mediaId}` with the access
/// token. Replaces the deprecated unauthenticated `/_matrix/media/v3/download`.
pub async fn download_media(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    access_token: &str,
    mxc: &str,
) -> Result<Vec<u8>, MatrixError> {
    let (server, media_id) = parse_mxc(mxc)?;
    let url = format!(
        "{api_base}/_matrix/client/v1/media/download/{}/{}",
        urlencoding::encode(server),
        urlencoding::encode(media_id),
    );
    let resp = http
        .get(&url)
        .bearer_auth(access_token)
        .timeout(MEDIA_DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|e| MatrixError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(MatrixError::Http { status, body });
    }
    // Stream the body with a hard cap so a homeserver that omits/lies about
    // Content-Length on an attacker-supplied mxc:// URI cannot OOM the host.
    let bytes = wcore_egress::read_body_capped(resp, MAX_MEDIA_BYTES)
        .await
        .map_err(|e| MatrixError::Network(format!("media body read: {e}")))?;
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Minimal urlencoding without adding a dep (percent-encode room IDs).
// ---------------------------------------------------------------------------
mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 4);
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
    mod tests {
        use super::*;
        #[test]
        fn encodes_exclamation_and_colon() {
            let encoded = encode("!room:example.org");
            assert_eq!(encoded, "%21room%3Aexample.org");
        }
    }
}

#[cfg(test)]
mod ack_tests {
    use super::*;

    const TOKEN: &str = "syt_test";
    const ROOM: &str = "!room123:example.org";

    #[tokio::test]
    async fn send_typing_puts_to_typing_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "PUT",
                "/_matrix/client/v3/rooms/%21room123%3Aexample.org/typing/%40bot%3Aexample.org",
            )
            .match_header("authorization", format!("Bearer {TOKEN}").as_str())
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        send_typing(
            &http,
            &server.url(),
            TOKEN,
            ROOM,
            "@bot:example.org",
            30_000,
        )
        .await
        .expect("typing should succeed on 200");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn send_reaction_puts_annotation_event() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(
                    r"/_matrix/client/v3/rooms/[^/]+/send/m\.reaction/\d+".to_string(),
                ),
            )
            .match_header("authorization", format!("Bearer {TOKEN}").as_str())
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex(r#""rel_type":"m\.annotation""#.to_string()),
                mockito::Matcher::Regex(r#""event_id":"\$evt1""#.to_string()),
                mockito::Matcher::Regex(r#""key":"👀""#.to_string()),
            ]))
            .with_status(200)
            .with_body(r#"{"event_id":"$react1"}"#)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        send_reaction(&http, &server.url(), TOKEN, ROOM, "$evt1", "👀")
            .await
            .expect("reaction should succeed on 200");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn reaction_http_error_surfaces() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("PUT", mockito::Matcher::Any)
            .with_status(403)
            .with_body(r#"{"errcode":"M_FORBIDDEN"}"#)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let err = send_reaction(&http, &server.url(), TOKEN, ROOM, "$evt1", "👀")
            .await
            .expect_err("403 should error");
        assert!(
            matches!(err, MatrixError::Http { status: 403, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_mxc_splits_server_and_id() {
        assert_eq!(
            parse_mxc("mxc://ex.org/abc123").unwrap(),
            ("ex.org", "abc123")
        );
        assert!(parse_mxc("https://ex.org/x").is_err());
        assert!(parse_mxc("mxc://ex.org/").is_err());
        assert!(parse_mxc("mxc://").is_err());
    }

    #[tokio::test]
    async fn download_media_uses_authenticated_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/_matrix/client/v1/media/download/ex.org/abc123")
            .match_header("authorization", format!("Bearer {TOKEN}").as_str())
            .with_status(200)
            .with_body(b"\x89PNG\r\n\x1a\nmatrixpng".as_slice())
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let bytes = download_media(&http, &server.url(), TOKEN, "mxc://ex.org/abc123")
            .await
            .expect("download should succeed");
        assert_eq!(&bytes[..4], b"\x89PNG");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn download_media_http_error_surfaces() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(404)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let err = download_media(&http, &server.url(), TOKEN, "mxc://ex.org/x")
            .await
            .expect_err("404 should error");
        assert!(
            matches!(err, MatrixError::Http { status: 404, .. }),
            "got {err:?}"
        );
    }
}
